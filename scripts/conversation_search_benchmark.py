#!/usr/bin/env python3
"""Private, opt-in conversation search benchmark helper.

This module deliberately uses only stdlib. It captures SQLite with the online
backup API and never opens the production database writable.
"""
from __future__ import annotations
import argparse
import hashlib
import json
import os
import re
import shutil
import sqlite3
import subprocess
import sys
import time
from pathlib import Path

DEFAULT_ARTIFACTS = Path("conversation-search-benchmark")
# Every named slow call is retained; duplicate IDs differing only in case are
# rejected rather than silently collapsing two production observations.
CALL_IDS = ["call_w7yaFJY51rJxTjog4DKeE2wo"]


def observed_call_ids():
    """Allow the operator to add independently verified call IDs without code edits."""
    configured = os.environ.get("PHOENIX_SEARCH_CALL_IDS", "")
    return list(dict.fromkeys(CALL_IDS + [item for item in configured.split(",") if item]))


DEFAULT_QUERIES = ['production-observed-slow-query', 'broad-common-term', 'selective-uncommon-term', 'verified-no-hit']

def _uri(path: Path) -> str:
    return f"file:{path.resolve()}?mode=ro"

def _hash(path: Path) -> str:
    h = hashlib.sha256()
    with path.open('rb') as f:
        for block in iter(lambda: f.read(1024 * 1024), b''):
            h.update(block)
    return h.hexdigest()

def _json_strings(value):
    """Yield query-shaped strings in a single tool block only."""
    if isinstance(value, dict):
        for key in ("query", "input", "arguments"):
            candidate = value.get(key)
            if isinstance(candidate, str):
                yield candidate
            elif isinstance(candidate, (dict, list)):
                yield from _json_strings(candidate)
        # Do not recurse through arbitrary sibling blocks: a message may contain
        # several tool calls and recovering a query from the wrong one is worse
        # than refusing to make a baseline.
    elif isinstance(value, list):
        for item in value:
            yield from _json_strings(item)


def _tool_block_ids(value):
    if isinstance(value, dict):
        for key in ("tool_use_id", "tool_call_id", "id"):
            candidate = value.get(key)
            if isinstance(candidate, str):
                yield candidate
        for item in value.values():
            if isinstance(item, (dict, list)):
                yield from _tool_block_ids(item)
    elif isinstance(value, list):
        for item in value:
            yield from _tool_block_ids(item)


def _recover_queries(conn: sqlite3.Connection) -> list[dict]:
    found = []
    seen_ids = set()
    for call_id in observed_call_ids():
        try:
            rows = conn.execute(
                "SELECT conversation_id,message_id,content,display_data FROM messages "
                "WHERE content LIKE ? OR display_data LIKE ? ORDER BY created_at, message_id",
                (f"%{call_id}%", f"%{call_id}%"),
            ).fetchall()
        except sqlite3.OperationalError as error:
            if "no such table: messages" in str(error):
                return found
            rows = conn.execute(
                "SELECT NULL,message_id,content,display_data FROM messages "
                "WHERE content LIKE ? OR display_data LIKE ? ORDER BY message_id",
                (f"%{call_id}%", f"%{call_id}%"),
            ).fetchall()
        for conversation_id, message_id, content, display_data in rows:
            for raw in (content, display_data):
                try:
                    parsed = json.loads(raw) if raw else None
                except (TypeError, ValueError):
                    continue
                # Only inspect the exact object/list containing this call id.
                # Sibling tool blocks in the same message must not contribute.
                def matching_blocks(value):
                    if isinstance(value, dict):
                        direct_ids = {
                            value.get(key)
                            for key in ("tool_use_id", "tool_call_id", "id")
                            if isinstance(value.get(key), str)
                        }
                        if call_id in direct_ids:
                            yield value
                        else:
                            for item in value.values():
                                yield from matching_blocks(item)
                    elif isinstance(value, list):
                        for item in value:
                            yield from matching_blocks(item)

                for block in matching_blocks(parsed):
                    query = next(iter(_json_strings(block)), None)
                    if query and query.strip() and call_id.lower() not in seen_ids:
                        found.append(
                            {
                                "query": query,
                                "source_call_id": call_id,
                                "conversation_id": conversation_id,
                                "message_id": message_id,
                            }
                        )
                        seen_ids.add(call_id.lower())
                        break
    return found

def _counts(conn):
    out = {}
    for table in ('conversations', 'messages', 'message_fts_rows'):
        try: out[table] = conn.execute(f'SELECT COUNT(*) FROM {table}').fetchone()[0]
        except sqlite3.Error: out[table] = None
    try: out['fts_rows'] = conn.execute('SELECT COUNT(*) FROM message_fts').fetchone()[0]
    except sqlite3.Error: out['fts_rows'] = None
    return out

def snapshot(args) -> int:
    source = Path(args.source).expanduser().resolve()
    outdir = Path(args.artifacts).expanduser().resolve()
    requested = Path(args.output).expanduser().resolve() if args.output else outdir / "captured.db"
    if (
        source == requested
        or source == outdir
        or source in outdir.parents
        or outdir in source.parents
    ):
        raise SystemExit("refusing to overwrite or capture from the artifact directory")
    if not source.exists(): raise SystemExit(f'source does not exist: {source}')
    outdir.mkdir(mode=0o700, parents=True, exist_ok=True)
    dest = requested
    if dest.exists() and not args.force: raise SystemExit(f'fixture exists (use --force only to replace): {dest}')
    if shutil.disk_usage(outdir).free < source.stat().st_size * 2:
        raise SystemExit('insufficient free space for snapshot and safety margin')
    tmp = outdir / f'.captured.db.{os.getpid()}.tmp'
    tmp.unlink(missing_ok=True)
    started = time.time()
    deadline_seconds = getattr(args, "deadline", 300.0)
    deadline = time.monotonic() + deadline_seconds
    last = None
    progress = {"pages": 0, "remaining": 0, "total": 0}
    for attempt in range(args.retries):
        if time.monotonic() >= deadline:
            break
        src = dst = None
        try:
            src = sqlite3.connect(_uri(source), uri=True, timeout=args.busy_timeout)
            dst = sqlite3.connect(tmp, timeout=args.busy_timeout)
            def on_progress(status, remaining, total):
                progress.update(pages=total - remaining, remaining=remaining, total=total)
                if time.monotonic() >= deadline:
                    raise TimeoutError("online backup deadline exceeded")
            with dst:
                src.backup(dst, pages=256, sleep=0.05, progress=on_progress)
            last = None
            break
        except (sqlite3.Error, TimeoutError) as error:
            last = error
            tmp.unlink(missing_ok=True)
            if time.monotonic() < deadline:
                time.sleep(min(2**attempt, 8, max(0, deadline - time.monotonic())))
        finally:
            if src is not None:
                src.close()
            if dst is not None:
                dst.close()
    if last or not tmp.exists():
        raise SystemExit(f"online backup failed before deadline ({deadline_seconds}s): {last}")
    # Integrity is checked while the fixture is still private and temporary.
    # Only a verified complete backup may become the published fixture.
    conn = sqlite3.connect(_uri(tmp), uri=True, timeout=args.busy_timeout)
    integrity = conn.execute("PRAGMA integrity_check").fetchone()[0]
    if integrity != "ok":
        conn.close()
        tmp.unlink(missing_ok=True)
        raise SystemExit(f"snapshot integrity check failed: {integrity}")
    counts = _counts(conn)
    recovered_queries = _recover_queries(conn)
    conn.close()
    os.replace(tmp, dest)
    os.chmod(dest, 0o600)
    manifest = {'kind':'conversation-search-fixture','source_path_not_retained':True,
      'captured_at_unix':started,'snapshot_path':str(dest),'size_bytes':dest.stat().st_size,
      'sha256':_hash(dest),'integrity_check':integrity,'sqlite_version':sqlite3.sqlite_version,
      'counts':counts,'recovered_queries':recovered_queries,'backup_progress':progress,
      'backup_deadline_seconds':deadline_seconds}
    (outdir/'capture-manifest.json').write_text(json.dumps(manifest, indent=2)+'\n'); os.chmod(outdir/'capture-manifest.json',0o600)
    print(f'captured immutable fixture: {dest}\nsha256: {manifest["sha256"]}\ncounts: {manifest["counts"]}')
    print(f'next: ./dev.py conversation-search prepare --artifacts {outdir}')
    return 0

def prepare(args) -> int:
    outdir = Path(args.artifacts).expanduser().resolve(); db = outdir/'captured.db'; manifest = outdir/'capture-manifest.json'
    if not db.exists() or not manifest.exists(): raise SystemExit('capture-manifest.json and captured.db are required')
    conn=sqlite3.connect(_uri(db), uri=True); recovered=_recover_queries(conn)
    if not recovered: raise SystemExit('named production call query was not recovered; refusing to invent a baseline')
    if len(recovered) < 2:
        raise SystemExit(
            "fewer than two independently identified slow calls were recovered; "
            "set PHOENIX_SEARCH_CALL_IDS to verified IDs rather than duplicating a query"
        )
    exact=recovered[0]['query']; ids=[]
    row=conn.execute('SELECT conversation_id FROM messages WHERE message_id=?',(recovered[0]['message_id'],)).fetchone()
    if row: ids=[row[0]]
    selective_terms = [term for term in re.findall(r"[A-Za-z0-9_]{6,}", exact) if term.lower() not in {"conversation", "search"}]
    selective_query = selective_terms[0] if selective_terms else "conversation"
    scenarios=[
      {'id':'observed-slow-exact','kind':'tool','query':exact,'source_call_id':recovered[0]['source_call_id']},
      {'id':'observed-slow-other','kind':'tool','query':recovered[1]['query'],'source_call_id':recovered[1]['source_call_id']},
      {'id':'broad-common','kind':'tool','query':'conversation'},
      {'id':'selective-known-match','kind':'tool','query':selective_query},
      {'id':'verified-no-hit','kind':'tool','query':'phoenix_benchmark_no_such_term_9f3c2'},
      {'id':'scoped-existing-transcript','kind':'retriever','query':exact,'conversation_ids':ids},
    ]
    conn.close(); (outdir/'scenarios.json').write_text(json.dumps({'version':1,'scenarios':scenarios},indent=2)+'\n'); os.chmod(outdir/'scenarios.json',0o600)
    print(f'wrote frozen scenarios: {outdir/"scenarios.json"}'); return 0

def run(args) -> int:
    outdir=Path(args.artifacts).expanduser().resolve(); db=outdir/'captured.db'; scen=outdir/'scenarios.json'
    if not db.exists() or not scen.exists(): raise SystemExit('run requires captured.db and scenarios.json')
    result_dir=outdir/'runs'; result_dir.mkdir(mode=0o700,exist_ok=True)
    cmd=['cargo','test','-p','phoenix_ide','--release','production_conversation_search_benchmark','--lib','--','--ignored','--nocapture']
    env=dict(os.environ,PHOENIX_SEARCH_BENCH_DB=str(db),PHOENIX_SEARCH_BENCH_SCENARIOS=str(scen),PHOENIX_SEARCH_BENCH_OUT=str(result_dir/f'{args.label}.json'))
    subprocess.run(cmd,cwd=Path(__file__).parents[1],env=env,check=True); print(env['PHOENIX_SEARCH_BENCH_OUT']); return 0

def _median(values):
    values = sorted(values)
    if not values:
        return None
    middle = len(values) // 2
    return values[middle] if len(values) % 2 else (values[middle - 1] + values[middle]) / 2


def _iqr(values):
    values = sorted(values)
    if len(values) < 2:
        return None
    middle = len(values) // 2
    lower, upper = values[:middle], values[(len(values) + 1) // 2:]
    return _median(upper) - _median(lower)


def report(args) -> int:
    outdir = Path(args.artifacts).expanduser().resolve()
    files = sorted((outdir / "runs").glob("*.json"))
    if not files:
        raise SystemExit("no run results")
    runs = [json.loads(path.read_text()) for path in files]
    rows = [sample for run in runs for sample in run.get("samples", [])]
    by = {}
    for sample in rows:
        by.setdefault((sample["case_id"], sample.get("phase", "unknown")), []).append(sample)
    lines = ["# Conversation search benchmark report", "", f"raw files: {len(files)}", ""]
    for (case, phase), values in by.items():
        durations = [x["duration_ms"] for x in values if x.get("ok")]
        errors = [x.get("error", x.get("result")) for x in values if not x.get("ok")]
        lines += [f"## {case} — {phase}", f"samples: {len(values)} successful: {len(durations)}"]
        if durations:
            lines.append(
                f"median_ms: {_median(durations):.3f} min_ms: {min(durations):.3f} "
                f"max_ms: {max(durations):.3f} iqr_ms: {_iqr(durations) or 0:.3f}"
            )
        if errors:
            lines.append(f"errors: {errors!r}")
        lines.append("")
    (outdir / "report.md").write_text("\n".join(lines))
    print(outdir / "report.md")
    return 0

def compare(args) -> int:
    a = json.loads(Path(args.before).read_text())
    b = json.loads(Path(args.after).read_text())
    keys = ("fixture_sha256", "scenario_digest", "profile", "warmup_runs", "measured_warm_runs")
    if any(a.get(key) != b.get(key) for key in keys):
        raise SystemExit("refusing comparison: fixture, scenarios, profile, or measurement regime differ")
    phases_a = {sample.get("phase") for sample in a.get("samples", [])}
    phases_b = {sample.get("phase") for sample in b.get("samples", [])}
    if phases_a != phases_b:
        raise SystemExit("refusing comparison: cold/warm measurement regimes differ")
    print("comparable fixture/scenarios/profile/regimes; see reports for raw distributions")
    return 0

def main():
    p=argparse.ArgumentParser(); sub=p.add_subparsers(dest='command',required=True)
    s=sub.add_parser("snapshot")
    s.add_argument("--source", default=os.environ.get("PHOENIX_PROD_DB", str(Path.home()/".phoenix-ide/prod.db")))
    s.add_argument("--artifacts", default=str(DEFAULT_ARTIFACTS))
    s.add_argument("--output", default="")
    s.add_argument("--force", action="store_true")
    s.add_argument("--retries", type=int, default=5)
    s.add_argument("--busy-timeout", type=float, default=5.0)
    s.add_argument("--deadline", type=float, default=300.0)
    s.set_defaults(func=snapshot)
    for name in ('prepare','run','report'):
      x=sub.add_parser(name); x.add_argument('--artifacts',default=str(DEFAULT_ARTIFACTS)); x.set_defaults(func=globals()[name])
    sub.choices['run'].add_argument('--label',default='suite-1')
    c=sub.add_parser('compare'); c.add_argument('before'); c.add_argument('after'); c.set_defaults(func=compare)
    args = p.parse_args()
    return args.func(args)
if __name__=='__main__': main()
