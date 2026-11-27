#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Measure what non-tool ("injected") messages cost across Phoenix conversations.

Read-only. Opens the database with `mode=ro` (or copies it first with
`--snapshot`) and never writes to it. See injected_context_audit.md for the
intent, the questions this answers, and how to extend it.

Model: every message the model sees is re-sent on every later request of the
same transcript until the transcript ends (continuation starts a new one).
Tool results from re-runnable reads may be cleared earlier (stale-tool-results);
injected user-role messages never are. For each message we estimate its tokens
and count the requests that carried it ("carries"), giving token-turns.
"""

from __future__ import annotations

import argparse
import bisect
import collections
import json
import sqlite3
import statistics
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path

# Mirrors crates/phoenix-ide/src/runtime/executor.rs IMAGE_TOKEN_ESTIMATE.
IMAGE_TOKENS = 1500
# Fixed guidance prepended to a continuation summary
# (send_chat_service::generated_predecessor_context_projection), in bytes.
CONTINUATION_PREAMBLE_BYTES = 1400
QUESTION_NO_LONGER_PENDING = "The question this event reported is no longer pending"

INJECTED = {
    "skill", "subscription_event", "scheduled_reminder", "cross_conversation",
    "slack_gateway", "subagent_summary", "followup_approval", "system_other",
    "continuation", "remote_api",
}


@dataclass
class Msg:
    conv: str
    seq: int
    created_at: str
    category: str
    detail: str
    bytes: int
    images: int
    key: str | None = None  # dedup/supersession key (skill name, watch source)
    tokens: float = 0.0
    carries: int = 0
    carries_after: dict = field(default_factory=dict)


@dataclass
class Req:
    seq: int
    created_at: str
    fresh: int
    cache_write: int
    cache_read: int
    output: int

    @property
    def total_in(self) -> int:
        return self.fresh + self.cache_write + self.cache_read


def open_db(path: Path, snapshot: bool) -> sqlite3.Connection:
    src = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    if not snapshot:
        return src
    tmp = Path(tempfile.mkdtemp(prefix="phoenix-audit-")) / "snapshot.db"
    dst = sqlite3.connect(tmp)
    src.backup(dst)
    src.close()
    print(f"(snapshot copied to {tmp})", file=sys.stderr)
    return dst


def jload(text: str | None):
    if not text:
        return None
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        return None


def classify(row, tool_names: dict[str, str], images_by_msg: dict[str, int], watch_key):
    """Return a Msg for a model-visible row, or None if the model never sees it."""
    (mid, conv, seq, mtype, content, display, created_at, origin, sub_event) = row
    data = jload(content)
    disp = jload(display) or {}
    imgs = images_by_msg.get(mid, 0)

    if mtype in ("system", "error"):
        return None
    if mtype == "agent":
        blocks = data if isinstance(data, list) else []
        for b in blocks:
            if isinstance(b, dict) and b.get("type") == "tool_use":
                tool_names[b.get("id", "")] = b.get("name", "?")
        return Msg(conv, seq, created_at, "assistant", "", len(content.encode()), 0)
    if mtype == "tool":
        d = data or {}
        name = tool_names.get(d.get("tool_use_id", ""), "?")
        body = d.get("content", "") or ""
        timgs = len(d.get("images") or [])
        return Msg(conv, seq, created_at, "tool_result", name, len(body.encode()), timgs)
    if mtype == "skill":
        d = data or {}
        body = d.get("body", "") or ""
        name = d.get("name", "?")
        return Msg(conv, seq, created_at, "skill", name, len(body.encode()) + 60, 0, key=f"skill:{name}")
    if mtype == "continuation":
        summary = (data or {}).get("summary", "") or ""
        return Msg(conv, seq, created_at, "continuation", "", len(summary.encode()) + CONTINUATION_PREAMBLE_BYTES, 0)
    if mtype != "user":
        return Msg(conv, seq, created_at, "other", mtype, len(content.encode()), 0)

    d = data or {}
    if d.get("is_meta") and disp.get("type") == "wake_result" and not disp.get("adopted"):
        return None  # skipped by render_messages
    text = d.get("llm_text") or d.get("text") or ""
    nbytes = len(text.encode()) + 60  # provenance header approximation
    first = text.lstrip()[:80].replace("\n", " ")
    if origin in ("user_api", "coordinator_question_reply"):
        cat = "user_typed" if not d.get("is_meta") else "system_other"
    elif origin == "subscription_event":
        cat = "subscription_event"
    elif origin == "scheduled_reminder":
        cat = "scheduled_reminder"
    elif origin in ("internal_conversation", "remote_conversation", "remote_coordinator"):
        cat = "cross_conversation"
    elif origin == "slack_gateway":
        cat = "slack_gateway"
    elif origin == "remote_api":
        cat = "remote_api"
    elif origin == "system_generated":
        if text.startswith("Sub-agent results ("):
            cat = "subagent_summary"
        elif text.startswith("Follow-up task approved"):
            cat = "followup_approval"
        else:
            cat = "system_other"
    else:
        cat = "user_typed" if not d.get("is_meta") else "system_other"
    key = watch_key(sub_event) if cat == "subscription_event" else None
    return Msg(conv, seq, created_at, cat, first, nbytes, imgs + len(d.get("images") or []), key=key)


def fit_tokens_per_byte(points: list[tuple[float, int]]):
    """Least squares total_in ≈ a + b·bytes. Returns (b, a) or None."""
    if len(points) < 4:
        return None
    xs = [p[0] for p in points]
    ys = [p[1] for p in points]
    mx, my = statistics.fmean(xs), statistics.fmean(ys)
    sxx = sum((x - mx) ** 2 for x in xs)
    if sxx == 0:
        return None
    b = sum((x - mx) * (y - my) for x, y in zip(xs, ys)) / sxx
    a = my - b * mx
    if not (0.05 < b < 1.0):
        return None
    return b, a


def pct(values, q):
    if not values:
        return 0
    values = sorted(values)
    k = min(len(values) - 1, max(0, round(q * (len(values) - 1))))
    return values[k]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("db", nargs="?", default=str(Path.home() / ".phoenix-ide" / "prod.db"))
    ap.add_argument("--snapshot", action="store_true", help="copy the DB first (safe with a live server)")
    ap.add_argument("--since", help="only transcripts created at/after this ISO date")
    ap.add_argument("--top", type=int, default=25)
    ap.add_argument("--json", help="also write machine-readable results here")
    ap.add_argument("--cache-read-mult", type=float, default=0.1)
    ap.add_argument("--cache-write-mult", type=float, default=1.25)
    ap.add_argument("--burst-seconds", type=float, default=60.0)
    ap.add_argument("--large-bytes", type=int, default=4096)
    ap.add_argument("--default-tokens-per-byte", type=float, default=0.27)
    args = ap.parse_args()

    conn = open_db(Path(args.db).expanduser(), args.snapshot)
    cur = conn.cursor()

    conv_rows = cur.execute(
        "SELECT id, runtime_role, coordinator_head, continued_in_conv_id, created_at, clear_watermark FROM conversations"
    ).fetchall()
    convs = {r[0]: {"role": r[1], "coord": r[2], "continued": r[3], "created": r[4], "watermark": r[5] or 0}
             for r in conv_rows if not args.since or (r[4] or "") >= args.since}

    images_by_msg: dict[str, int] = collections.Counter(
        r[0] for r in cur.execute("SELECT message_id FROM message_images")
    )
    watch = {r[0]: (r[1], r[2]) for r in cur.execute(
        "SELECT event_id, watch_id, source_transcript_id FROM coordinator_watch_events")}

    def watch_key(event_id):
        w = watch.get(event_id)
        return f"watch:{w[0]}:{w[1]}" if w else None

    msgs: dict[str, list[Msg]] = collections.defaultdict(list)
    reqs: dict[str, list[Req]] = collections.defaultdict(list)
    tool_names: dict[str, str] = {}
    hidden = 0
    for row in cur.execute(
        "SELECT message_id, conversation_id, sequence_id, message_type, content, display_data,"
        " created_at, origin_kind, origin_subscription_event_id, usage_data"
        " FROM messages ORDER BY conversation_id, sequence_id"
    ):
        if row[1] not in convs:
            continue
        m = classify(row[:9], tool_names, images_by_msg, watch_key)
        if m is None:
            hidden += 1
        else:
            msgs[row[1]].append(m)
        usage = jload(row[9]) if row[3] == "agent" else None
        if isinstance(usage, dict) and "input_tokens" in usage:
            reqs[row[1]].append(Req(row[2], row[6], int(usage.get("input_tokens") or 0),
                                    int(usage.get("cache_creation_tokens") or 0),
                                    int(usage.get("cache_read_tokens") or 0),
                                    int(usage.get("output_tokens") or 0)))

    # --- calibration: per-transcript tokens/byte from request inputs -------------
    fits = {}
    for conv, rs in reqs.items():
        ms = msgs.get(conv, [])
        pts = []
        j = b = im = 0
        for r in rs:  # both lists are in sequence order: prefix sums, one pass
            while j < len(ms) and ms[j].seq < r.seq:
                b += ms[j].bytes
                im += ms[j].images
                j += 1
            pts.append((b, r.total_in - im * IMAGE_TOKENS))
        f = fit_tokens_per_byte(pts)
        if f:
            fits[conv] = f
    slopes = [f[0] for f in fits.values()]
    global_tpb = statistics.median(slopes) if slopes else args.default_tokens_per_byte
    overheads = [f[1] for f in fits.values()]

    # --- carries -----------------------------------------------------------------
    for conv, ms in msgs.items():
        tpb = fits.get(conv, (global_tpb, 0))[0]
        rseqs = [r.seq for r in reqs.get(conv, [])]
        for m in ms:
            m.tokens = m.bytes * tpb + m.images * IMAGE_TOKENS
            m.carries = len(rseqs) - bisect.bisect_right(rseqs, m.seq)

    def token_turns(m):
        return m.tokens * m.carries

    def cache_weighted(m):
        if m.carries == 0:
            return 0.0
        return m.tokens * (args.cache_write_mult + (m.carries - 1) * args.cache_read_mult)

    all_msgs = [m for ms in msgs.values() for m in ms]
    all_reqs = [r for rs in reqs.values() for r in rs]
    total_tt = sum(token_turns(m) for m in all_msgs) or 1.0
    out: dict = {"params": vars(args)}

    def p(line=""):
        print(line)

    # 1. corpus -------------------------------------------------------------------
    p("# Injected-context audit\n")
    p("## 1. Corpus")
    p(f"- transcripts: {len(convs)} (with ≥1 request: {sum(1 for c in reqs if reqs[c])}; "
      f"coordinator: {sum(1 for c in convs.values() if c['coord'] or (c['role'] or '').startswith('coordinator'))})")
    p(f"- model-visible messages: {len(all_msgs)}; hidden (system/error/unadopted wake): {hidden}")
    p(f"- requests: {len(all_reqs)}; input tokens fresh/cache-write/cache-read: "
      f"{sum(r.fresh for r in all_reqs):,} / {sum(r.cache_write for r in all_reqs):,} / {sum(r.cache_read for r in all_reqs):,}")
    p(f"- date range: {min((c['created'] or '') for c in convs.values()) if convs else '-'} .. "
      f"{max((c['created'] or '') for c in convs.values()) if convs else '-'}")
    out["corpus"] = {"transcripts": len(convs), "messages": len(all_msgs), "requests": len(all_reqs)}

    # 2. calibration ----------------------------------------------------------------
    p("\n## 2. Calibration")
    p(f"- transcripts with a usable fit: {len(fits)}; median tokens/byte {global_tpb:.3f} "
      f"(p10 {pct(slopes, .1):.3f}, p90 {pct(slopes, .9):.3f})")
    p(f"- median fixed per-request overhead (system prompt + tools): {statistics.median(overheads) if overheads else 0:,.0f} tokens")
    p("- Token figures below are estimates from this fit; treat them as ±25%.")
    out["calibration"] = {"tokens_per_byte": global_tpb, "fits": len(fits),
                          "fixed_overhead_median": statistics.median(overheads) if overheads else None}

    # 3. category table -------------------------------------------------------------
    p("\n## 3. Carry cost by category")
    p("| category | msgs | est tokens | p50 | p95 | token-turns | share | cache-weighted |")
    p("|---|---:|---:|---:|---:|---:|---:|---:|")
    by_cat = collections.defaultdict(list)
    for m in all_msgs:
        by_cat[m.category].append(m)
    cats = []
    for cat, ms in sorted(by_cat.items(), key=lambda kv: -sum(token_turns(m) for m in kv[1])):
        toks = [m.tokens for m in ms]
        tt = sum(token_turns(m) for m in ms)
        cw = sum(cache_weighted(m) for m in ms)
        flag = " (upper bound: some cleared)" if cat == "tool_result" else ""
        p(f"| {cat}{flag} | {len(ms)} | {sum(toks):,.0f} | {pct(toks, .5):,.0f} | {pct(toks, .95):,.0f} | "
          f"{tt:,.0f} | {100 * tt / total_tt:.1f}% | {cw:,.0f} |")
        cats.append({"category": cat, "msgs": len(ms), "tokens": sum(toks), "token_turns": tt, "cache_weighted": cw})
    out["categories"] = cats

    def is_coord(conv):
        c = convs.get(conv, {})
        return bool(c.get("coord")) or c.get("role") == "coordinator"

    p("\n### 3b. Same, split by transcript kind (share of that kind's token-turns)")
    for label, pick in (("coordinator", True), ("ordinary + sub-agent", False)):
        kind = [m for m in all_msgs if is_coord(m.conv) == pick]
        ktt = sum(token_turns(m) for m in kind) or 1.0
        agg = collections.Counter()
        for m in kind:
            agg[m.category] += token_turns(m)
        top_k = ", ".join(f"{c} {100 * v / ktt:.1f}%" for c, v in agg.most_common(6))
        p(f"- {label} ({len({m.conv for m in kind})} transcripts): {top_k or 'none'}")
        out.setdefault("by_kind", {})[label] = {c: v / ktt for c, v in agg.items()}

    # 4. composition at the final request -------------------------------------------
    p("\n## 4. Composition of the last request in each transcript (mean share of message tokens)")
    shares = collections.defaultdict(list)
    for conv, rs in reqs.items():
        if not rs:
            continue
        last = rs[-1].seq
        present = [m for m in msgs.get(conv, []) if m.seq < last]
        tot = sum(m.tokens for m in present) or 1
        agg = collections.Counter()
        for m in present:
            agg[m.category] += m.tokens
        for cat in by_cat:
            shares[cat].append(agg.get(cat, 0) / tot)
    for cat, vals in sorted(shares.items(), key=lambda kv: -statistics.fmean(kv[1])):
        p(f"- {cat}: mean {100 * statistics.fmean(vals):.1f}%, p90 {100 * pct(vals, .9):.1f}%")
    out["final_request_share"] = {k: statistics.fmean(v) for k, v in shares.items()}

    # 5. top individual injected messages -------------------------------------------
    p(f"\n## 5. Top {args.top} injected messages by token-turns")
    p("| transcript | seq | category | detail | tokens | carries | token-turns |")
    p("|---|---:|---|---|---:|---:|---:|")
    top = sorted((m for m in all_msgs if m.category in INJECTED), key=token_turns, reverse=True)[: args.top]
    for m in top:
        p(f"| {m.conv[:8]} | {m.seq} | {m.category} | {m.detail[:50].replace('|', '/')} | {m.tokens:,.0f} | {m.carries} | {token_turns(m):,.0f} |")
    out["top"] = [{"conv": m.conv, "seq": m.seq, "category": m.category, "detail": m.detail,
                   "tokens": m.tokens, "carries": m.carries} for m in top]

    # 6. redundancy -------------------------------------------------------------------
    p("\n## 6. Redundant carried content")
    by_seq = {conv: {m.seq: m for m in ms} for conv, ms in msgs.items()}
    redundant = collections.Counter()
    examples = collections.defaultdict(list)
    for conv, ms in msgs.items():
        rseqs = [r.seq for r in reqs.get(conv, [])]
        last_by_key: dict[str, Msg] = {}
        for m in ms:
            if m.key and m.key in last_by_key:
                prev = last_by_key[m.key]
                # the earlier copy is redundant on every request after the newer one arrives
                waste = prev.tokens * (len(rseqs) - bisect.bisect_right(rseqs, m.seq))
                kind = "duplicate_skill" if m.category == "skill" else "superseded_event"
                redundant[kind] += waste
                if len(examples[kind]) < 5:
                    examples[kind].append(f"{conv[:8]} seq {prev.seq}→{m.seq} ({m.detail[:30] or m.key})")
            if m.key:
                last_by_key[m.key] = m
    # detail holds only 80 chars; re-scan full text for the stale-question marker
    for (conv, seq, content) in cur.execute(
        "SELECT conversation_id, sequence_id, content FROM messages WHERE origin_kind='subscription_event'"
    ):
        if conv in by_seq and QUESTION_NO_LONGER_PENDING in (content or ""):
            m = by_seq[conv].get(seq)
            if m is not None:
                redundant["stale_question_event"] += token_turns(m)
    fa = [m for m in all_msgs if m.category == "followup_approval"]
    redundant["followup_plan_copy"] = sum(token_turns(m) for m in fa)
    for kind, tt in redundant.most_common():
        p(f"- {kind}: {tt:,.0f} token-turns ({100 * tt / total_tt:.2f}% of all)")
        for e in examples.get(kind, []):
            p(f"  - e.g. {e}")
    out["redundant"] = dict(redundant)

    # 7. wake amplification ---------------------------------------------------------
    p("\n## 7. Turns started by injected input (not by the user)")
    woke = collections.Counter()
    woke_tokens = collections.Counter()
    bursts = 0
    burst_tokens = 0
    from datetime import datetime

    def ts(s):
        try:
            return datetime.fromisoformat(s.replace("Z", "+00:00"))
        except (ValueError, AttributeError):
            return None

    for conv, ms in msgs.items():
        rs = reqs.get(conv, [])
        last_trigger = None
        trig = None
        j = 0
        for r in rs:
            while j < len(ms) and ms[j].seq < r.seq:
                if ms[j].category not in ("assistant", "tool_result"):
                    trig = ms[j]
                j += 1
            if trig is None or trig is last_trigger:
                continue
            last_trigger = trig
            # A skill message is typed by the user, so it is not an unprompted wake.
            if trig.category in INJECTED and trig.category != "skill":
                woke[trig.category] += 1
                woke_tokens[trig.category] += r.total_in
        events = [m for m in ms if m.category == "subscription_event"]
        for a, b in zip(events, events[1:]):
            ta, tb = ts(a.created_at), ts(b.created_at)
            if ta and tb and (tb - ta).total_seconds() <= args.burst_seconds:
                bursts += 1
                # a request answered between two close events is the one batching would avoid
                burst_tokens += sum(r.total_in for r in rs if a.seq < r.seq < b.seq)
    for cat, n in woke.most_common():
        p(f"- {cat}: {n} turns, {woke_tokens[cat]:,} input tokens re-sent")
    p(f"- subscription events within {args.burst_seconds:.0f}s of the previous one: {bursts} "
      f"(batching could avoid ≈{burst_tokens:,} input tokens)")
    out["wakes"] = {"turns": dict(woke), "input_tokens": dict(woke_tokens),
                    "burst_events": bursts, "burst_tokens": burst_tokens}

    # 8. continuation pressure ------------------------------------------------------
    p("\n## 8. Injected share at the point a transcript continued")
    vals = []
    for conv, c in convs.items():
        if not c["continued"] or not reqs.get(conv):
            continue
        last = reqs[conv][-1]
        inj = sum(m.tokens for m in msgs[conv] if m.seq < last.seq and m.category in INJECTED)
        vals.append((inj, last.total_in))
    if vals:
        fr = [i / t for i, t in vals if t]
        p(f"- continued transcripts: {len(vals)}; injected share of final input: mean {100 * statistics.fmean(fr):.1f}%, "
          f"p90 {100 * pct(fr, .9):.1f}%; mean injected tokens {statistics.fmean(i for i, _ in vals):,.0f}")
    else:
        p("- no continued transcripts with usage in range")
    out["continuation"] = vals

    # 9. cache behaviour after large injections ------------------------------------
    p(f"\n## 9. Cache behaviour on the request after an injected message ≥ {args.large_bytes} bytes")
    after, others = [], []
    for conv, ms in msgs.items():
        rs = reqs.get(conv, [])
        big = [m.seq for m in ms if m.category in INJECTED and m.bytes >= args.large_bytes]
        for i, r in enumerate(rs):
            prev_seq = rs[i - 1].seq if i else -1
            (after if any(prev_seq < s < r.seq for s in big) else others).append(r)

    def hit(rs):
        tot = sum(r.total_in for r in rs) or 1
        return 100 * sum(r.cache_read for r in rs) / tot, 100 * sum(r.cache_write for r in rs) / tot

    if after:
        a_hit, a_w = hit(after)
        o_hit, o_w = hit(others)
        p(f"- after large injection: {len(after)} requests, cache-read {a_hit:.1f}%, cache-write {a_w:.1f}% of input")
        p(f"- all other requests: {len(others)}, cache-read {o_hit:.1f}%, cache-write {o_w:.1f}%")
    else:
        p("- none in range")

    if args.json:
        Path(args.json).write_text(json.dumps(out, indent=1, default=str))
        print(f"\n(json written to {args.json})", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
