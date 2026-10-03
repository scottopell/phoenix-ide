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
import platform
import re
import shutil
import signal
import sqlite3
import subprocess
import time
from pathlib import Path

DEFAULT_ARTIFACTS = Path("conversation-search-benchmark")
# Every named slow call is retained; duplicate IDs differing only in case are
# rejected rather than silently collapsing two production observations. Known
# transcript ids let capture use the indexed conversation_id column rather than
# scanning every content blob in a multi-gigabyte database.
KNOWN_CALLS = [
    {"conversation_id": "9bc3b72d", "prefix": True, "call_id": "call_w7yaFJY51rJxTjog4DKeE2wo"},
    {"conversation_id": "ca640950-4b82-4499-a1af-9846827f29dc", "call_id": "call_AHuEHesog7J4anh2hSiia6kh"},
]
CALL_IDS = [item["call_id"] for item in KNOWN_CALLS]
RECOVERY_MESSAGE_LIMIT = 2_000
LABEL_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")


def _private(path: Path) -> None:
    """Create a private directory and reject symlinked artifact roots."""
    if path.exists() and path.is_symlink():
        raise SystemExit(f"refusing symlinked artifact directory: {path}")
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    os.chmod(path, 0o700)


def _label(label: str) -> str:
    if not LABEL_RE.fullmatch(label):
        raise SystemExit("label must contain only letters, digits, '.', '_' or '-' and be at most 64 characters")
    return label


def _write_private(path: Path, text: str) -> None:
    if path.exists() and path.is_symlink():
        raise SystemExit(f"refusing symlinked private artifact: {path}")
    path.write_text(text)
    os.chmod(path, 0o600)


def _git_commit() -> str:
    try:
        return subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=Path(__file__).parents[1], text=True
        ).strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def observed_call_ids():
    """Allow the operator to add independently verified call IDs without code edits."""
    configured = os.environ.get("PHOENIX_SEARCH_CALL_IDS", "")
    return list(dict.fromkeys(CALL_IDS + [item for item in configured.split(",") if item]))




def _uri(path: Path) -> str:
    return f"file:{path.resolve()}?mode=ro"

def _hash(path: Path) -> str:
    h = hashlib.sha256()
    with path.open('rb') as f:
        for block in iter(lambda: f.read(1024 * 1024), b''):
            h.update(block)
    return h.hexdigest()

def _json_strings(value):
    """Yield query strings from one tool block, decoding serialized arguments."""
    if isinstance(value, dict):
        for key in ("query", "input"):
            candidate = value.get(key)
            if isinstance(candidate, str):
                yield candidate
            elif isinstance(candidate, (dict, list)):
                yield from _json_strings(candidate)
        arguments = value.get("arguments")
        if isinstance(arguments, (dict, list)):
            yield from _json_strings(arguments)
        elif isinstance(arguments, str):
            try:
                decoded = json.loads(arguments)
            except (TypeError, ValueError):
                decoded = None
            if isinstance(decoded, (dict, list)):
                yield from _json_strings(decoded)
        # Do not recurse through arbitrary sibling blocks: a message may contain
        # several tool calls and recovering a query from the wrong one is worse
        # than refusing to make a baseline.
    elif isinstance(value, list):
        for item in value:
            yield from _json_strings(item)



def _message_columns(conn: sqlite3.Connection) -> set[str]:
    try:
        return {row[1] for row in conn.execute("PRAGMA table_info(messages)")}
    except sqlite3.Error:
        return set()


def _recover_from_rows(rows, call_id: str, found: list[dict], seen_ids: set[str]) -> None:
    for conversation_id, message_id, content, display_data in rows:
        for raw in (content, display_data):
            try:
                parsed = json.loads(raw) if raw else None
            except (TypeError, ValueError):
                continue

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
                    found.append({
                        "query": query,
                        "source_call_id": call_id,
                        "conversation_id": conversation_id,
                        "message_id": message_id,
                    })
                    seen_ids.add(call_id.lower())
                    return


def _recover_queries(conn: sqlite3.Connection) -> list[dict]:
    """Recover named calls using bounded indexed transcript lookups when known."""
    found: list[dict] = []
    seen_ids: set[str] = set()
    columns = _message_columns(conn)
    if not columns:
        return found

    if "conversation_id" in columns:
        # The first production id is intentionally a stable prefix; the second
        # is already a complete UUID. Both predicates remain indexable and are
        # bounded to a small transcript window.
        known_rows = False
        for item in KNOWN_CALLS:
            predicate = "conversation_id=?" if not item.get("prefix") else "conversation_id LIKE ?"
            value = item["conversation_id"] + "%" if item.get("prefix") else item["conversation_id"]
            if conn.execute(f"SELECT 1 FROM messages WHERE {predicate} LIMIT 1", (value,)).fetchone():
                known_rows = True
                break
        if known_rows:
            order = "created_at, message_id" if "created_at" in columns else "message_id"
            for item in KNOWN_CALLS:
                predicate = "conversation_id=?" if not item.get("prefix") else "conversation_id LIKE ?"
                value = item["conversation_id"] + "%" if item.get("prefix") else item["conversation_id"]
                rows = conn.execute(
                    f"SELECT conversation_id,message_id,content,display_data FROM messages "
                    f"WHERE {predicate} ORDER BY {order} LIMIT ?",
                    (value, RECOVERY_MESSAGE_LIMIT),
                ).fetchall()
                _recover_from_rows(rows, item["call_id"], found, seen_ids)
            return found

    # Synthetic fixtures and older schemas retain the LIKE fallback so tests can
    # exercise recovery without manufacturing production transcript ids.
    conversation = "conversation_id" if "conversation_id" in columns else "NULL"
    order = "created_at, message_id" if "created_at" in columns else "message_id"
    for call_id in observed_call_ids():
        rows = conn.execute(
            f"SELECT {conversation},message_id,content,display_data FROM messages "
            "WHERE content LIKE ? OR display_data LIKE ? "
            f"ORDER BY {order} LIMIT ?",
            (f"%{call_id}%", f"%{call_id}%", RECOVERY_MESSAGE_LIMIT),
        ).fetchall()
        _recover_from_rows(rows, call_id, found, seen_ids)
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
    _private(outdir)
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
    manifest = {'kind':'conversation-search-fixture','source_path':str(source),
      'captured_at_unix':started,'snapshot_path':str(dest),'size_bytes':dest.stat().st_size,
      'sha256':_hash(dest),'integrity_check':integrity,'sqlite_version':sqlite3.sqlite_version,
      'counts':counts,'recovered_queries':recovered_queries,'backup_progress':progress,
      'backup_deadline_seconds':deadline_seconds}
    _write_private(outdir/'capture-manifest.json', json.dumps(manifest, indent=2)+'\n')
    print(f'captured immutable fixture: {dest}\nsha256: {manifest["sha256"]}\ncounts: {manifest["counts"]}')
    print(f'next: ./dev.py conversation-search prepare --artifacts {outdir}')
    return 0

def prepare(args) -> int:
    outdir = Path(args.artifacts).expanduser().resolve(); db = outdir/'captured.db'; manifest = outdir/'capture-manifest.json'
    if not db.exists() or not manifest.exists(): raise SystemExit('capture-manifest.json and captured.db are required')
    capture = json.loads(manifest.read_text())
    if capture.get('kind') != 'conversation-search-fixture' or capture.get('snapshot_path') != str(db):
        raise SystemExit('capture manifest does not identify this captured.db')
    if capture.get('size_bytes') != db.stat().st_size or capture.get('sha256') != _hash(db):
        raise SystemExit('captured.db does not match capture manifest')
    if Path(capture.get('source_path', '')).resolve() == db:
        raise SystemExit('refusing to benchmark the capture source directly')
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
      {'id':'observed-slow-exact','kind':'tool','query':exact,'source_call_id':recovered[0]['source_call_id'],'expected':'hit'},
      {'id':'observed-slow-other','kind':'tool','query':recovered[1]['query'],'source_call_id':recovered[1]['source_call_id'],'expected':'hit'},
      {'id':'broad-common','kind':'tool','query':'conversation','expected':'hit'},
      {'id':'selective-known-match','kind':'tool','query':selective_query,'expected':'hit'},
      {'id':'verified-no-hit','kind':'tool','query':'phoenix_benchmark_no_such_term_9f3c2','expected':'no_hit'},
      {'id':'scoped-existing-transcript','kind':'retriever','scope':'conversation','query':exact,'conversation_ids':ids,'expected':'hit'},
    ]
    conn.close()
    scenarios_path = outdir/'scenarios.json'
    if scenarios_path.exists() and not getattr(args, "force", False):
        raise SystemExit(f'frozen scenarios exist (use --force only to replace): {scenarios_path}')
    _write_private(scenarios_path, json.dumps({'version':1,'scenarios':scenarios},indent=2)+'\n')
    print(f'wrote frozen scenarios: {scenarios_path}'); return 0

def run(args) -> int:
    outdir=Path(args.artifacts).expanduser().resolve(); db=outdir/'captured.db'; scen=outdir/'scenarios.json'
    manifest=outdir/'capture-manifest.json'
    if not db.exists() or not scen.exists() or not manifest.exists():
        raise SystemExit('run requires captured.db, capture-manifest.json, and scenarios.json')
    capture=json.loads(manifest.read_text())
    if capture.get('snapshot_path') != str(db) or capture.get('sha256') != _hash(db) or capture.get('size_bytes') != db.stat().st_size:
        raise SystemExit('captured.db does not match capture-manifest.json')
    if Path(capture.get('source_path', '')).resolve() == db:
        raise SystemExit('refusing to benchmark the capture source directly')
    scenarios=json.loads(scen.read_text())
    if scenarios.get('version') != 1 or not isinstance(scenarios.get('scenarios'), list):
        raise SystemExit('invalid scenarios manifest')
    result_dir=outdir/'runs'; _private(result_dir)
    label=_label(args.label)
    output=result_dir/f'{label}.json'
    if output.exists() and not args.force:
        raise SystemExit(f'result exists (use --force only to replace): {output}')
    cmd=['cargo','test','-p','phoenix_ide','--release','production_conversation_search_benchmark','--lib','--','--ignored','--nocapture']
    env=dict(os.environ,
        PHOENIX_SEARCH_BENCH_DB=str(db), PHOENIX_SEARCH_BENCH_SCENARIOS=str(scen),
        PHOENIX_SEARCH_BENCH_CAPTURE_MANIFEST=str(manifest), PHOENIX_SEARCH_BENCH_OUT=str(output),
        PHOENIX_SEARCH_BENCH_COMMIT=_git_commit(), PHOENIX_SEARCH_BENCH_HOST=platform.node(),
        PHOENIX_SEARCH_BENCH_PLATFORM=platform.platform(), PHOENIX_SEARCH_BENCH_PROCESSOR=platform.processor(),
        PHOENIX_SEARCH_BENCH_CPU_COUNT=str(os.cpu_count() or 1))
    process = subprocess.Popen(
        cmd,
        cwd=Path(__file__).parents[1],
        env=env,
        start_new_session=(os.name == "posix"),
    )
    try:
        process.wait(timeout=args.timeout)
    except subprocess.TimeoutExpired as error:
        if os.name == "posix":
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
        else:
            process.kill()
            process.wait()
        failure = {
            "kind": "conversation-search-benchmark-failure",
            "run_label": label,
            "status": "outer_timeout",
            "timeout_seconds": args.timeout,
            "command": cmd,
            "recorded_at_unix": time.time(),
        }
        failures_dir = result_dir / "failures"
        _private(failures_dir)
        _write_private(failures_dir/f"{label}.json", json.dumps(failure, indent=2) + "\n")
        raise SystemExit(f'benchmark timed out after {args.timeout}s; process group was stopped') from error
    if process.returncode:
        raise SystemExit(f'benchmark failed with exit status {process.returncode}')
    print(output); return 0

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
    runs = [(path, json.loads(path.read_text())) for path in files]
    rows = []
    for path, run in runs:
        run_label = run.get("run_label") or path.stem
        for sample in run.get("samples", []):
            # The Rust harness emits one warmup invocation per case/surface.
            # Keep failed warmups visible, but never let successful warmups
            # influence the report's timings or counts.
            if sample.get("phase") == "warmup_discarded" and sample.get("ok"):
                continue
            rows.append((run_label, path.name, sample))
    by = {}
    for run_label, filename, sample in rows:
        by.setdefault((run_label, filename, sample["case_id"], sample.get("surface", "unknown"), sample.get("phase", "unknown")), []).append(sample)
    lines = ["# Conversation search benchmark report", "", f"raw files: {len(files)}", ""]
    for (run_label, filename, case, surface, phase), values in by.items():
        durations = [x["duration_ms"] for x in values if x.get("ok")]
        errors = [x.get("error", x.get("result")) for x in values if not x.get("ok")]
        lines += [f"## {run_label} / {filename} — {case} — {surface} — {phase}", f"samples: {len(values)} successful: {len(durations)}"]
        if durations:
            lines.append(
                f"median_ms: {_median(durations):.3f} min_ms: {min(durations):.3f} "
                f"max_ms: {max(durations):.3f} iqr_ms: {_iqr(durations) or 0:.3f}"
            )
        if errors:
            lines.append(f"errors: {errors!r}")
        lines.append("")
    _write_private(outdir / "report.md", "\n".join(lines))
    print(outdir / "report.md")
    return 0

def _metadata_has_values(value) -> bool:
    if isinstance(value, dict):
        return bool(value) and all(_metadata_has_values(item) for item in value.values())
    if isinstance(value, (list, tuple)):
        return bool(value) and all(_metadata_has_values(item) for item in value)
    if isinstance(value, str):
        return bool(value.strip())
    return value is not None


def _validate_run(run: dict, name: str) -> dict:
    required = {"fixture_sha256", "scenario_digest", "profile", "warmup_runs", "measured_warm_runs", "commit", "environment", "sqlite_pragmas", "runtime", "explain_enabled", "explain_plans", "samples"}
    missing = sorted(required - run.keys())
    if missing:
        raise SystemExit(f"refusing comparison: {name} is missing metadata: {', '.join(missing)}")
    for key in required - {"samples", "explain_plans"}:
        if not _metadata_has_values(run[key]):
            raise SystemExit(f"refusing comparison: {name} has empty metadata: {key}")
    if run["explain_enabled"] and not _metadata_has_values(run["explain_plans"]):
        raise SystemExit(f"refusing comparison: {name} has empty metadata: explain_plans")
    if not isinstance(run["samples"], list) or not run["samples"]:
        raise SystemExit(f"refusing comparison: {name} has no samples")
    digests = {}
    phases = {}
    for sample in run["samples"]:
        key = (sample.get("case_id"), sample.get("surface"))
        digest = sample.get("result_digest")
        phase = sample.get("phase")
        if not key[0] or not key[1] or not digest or not phase:
            raise SystemExit(f"refusing comparison: {name} has incomplete sample output metadata")
        if sample.get("ok") is not True:
            raise SystemExit(f"refusing comparison: {name} has errors/timeouts for {key[0]} ({key[1]})")
        if not isinstance(sample.get("duration_ms"), (int, float)):
            raise SystemExit(f"refusing comparison: {name} has invalid duration for {key[0]} ({key[1]})")
        prior = digests.setdefault(key, digest)
        if prior != digest:
            raise SystemExit(f"refusing comparison: output mismatch for {key[0]} ({key[1]}) in {name}")
        phases.setdefault(key, {}).setdefault(phase, []).append(sample)
    expected_warm = run["measured_warm_runs"]
    if expected_warm != 10:
        raise SystemExit(f"refusing comparison: {name} must declare exactly 10 measured warm runs")
    for key, by_phase in phases.items():
        warm = by_phase.get("warm", [])
        iterations = sorted(sample.get("iteration") for sample in warm)
        if len(warm) != 10 or iterations != list(range(10)):
            raise SystemExit(f"refusing comparison: {name} has incomplete warm iterations for {key[0]} ({key[1]})")
    return {"digests": digests, "phases": phases}


def compare(args) -> int:
    a = json.loads(Path(args.before).read_text())
    b = json.loads(Path(args.after).read_text())
    validated_a = _validate_run(a, "before")
    validated_b = _validate_run(b, "after")
    keys = ("fixture_sha256", "scenario_digest", "profile", "warmup_runs", "measured_warm_runs", "environment", "sqlite_pragmas", "runtime", "explain_enabled", "explain_plans")
    if any(a.get(key) != b.get(key) for key in keys):
        raise SystemExit("refusing comparison: fixture, scenarios, profile, or full measurement regime differ")
    if validated_a["phases"].keys() != validated_b["phases"].keys():
        raise SystemExit("refusing comparison: case/surface regimes differ")
    if validated_a["digests"] != validated_b["digests"]:
        digests_a, digests_b = validated_a["digests"], validated_b["digests"]
        raise SystemExit("refusing comparison: output mismatch (identity/digests differ between runs)")
    medians = []
    for run in (a, b):
        by = {}
        for sample in run["samples"]:
            if sample.get("phase") == "warm" and sample.get("ok"):
                by.setdefault((sample["case_id"], sample["surface"]), []).append(sample["duration_ms"])
        medians.append(by)
    print("comparable fixture/scenarios/profile/regimes; per-case warm medians (before -> after):")
    for key in sorted(set(medians[0]) | set(medians[1])):
        before = _median(medians[0].get(key, []))
        after = _median(medians[1].get(key, []))
        if before is None or after is None:
            raise SystemExit(f"refusing comparison: missing successful warm samples for {key[0]} ({key[1]})")
        print(f"  {key[0]} ({key[1]}): {before:.3f} -> {after:.3f} ms")
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
    sub.choices['prepare'].add_argument('--force',action='store_true')
    sub.choices['run'].add_argument('--label',default='suite-1')
    sub.choices['run'].add_argument('--timeout',type=float,default=1800.0)
    sub.choices['run'].add_argument('--force',action='store_true')
    c=sub.add_parser('compare'); c.add_argument('before'); c.add_argument('after'); c.set_defaults(func=compare)
    args = p.parse_args()
    return args.func(args)
if __name__=='__main__': main()
