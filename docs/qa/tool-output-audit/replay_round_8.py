#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.13"
# dependencies = []
# ///
"""Replay prospective bash output caps without exporting transcript content."""

from bisect import bisect_left, bisect_right
from collections import Counter, defaultdict
from contextlib import closing
from dataclasses import dataclass, field
from itertools import groupby
import argparse
import json
from pathlib import Path
import re
import shlex
import sys

from replay_round_3 import ExactTranscript, RUNTIME_TRUNCATION, baseline


KIB = 1024
CAPS = ((8, 2), (16, 4), (32, 8))
SAFE_BUILD_FAMILIES = {"cargo build", "cargo check", "cargo clippy", "cargo fmt"}
DIAGNOSTIC = re.compile(
    rb"\b(?:errors?|fail(?:ed|ure)|panicked|traceback|assert(?:_[a-z_]+)?)\b|\bwarning:|"
    + "✗".encode(), re.I
)
TOKEN = re.compile(rb"[A-Za-z0-9_.~/][A-Za-z0-9_.~/@:+-]*")


def meaningful(token):
    token = token.strip(b".:+-")
    minimum = 4 if token.isdigit() else 6
    return token if len(token) >= minimum else None


def tokens(data):
    return {value for hit in TOKEN.finditer(data) if (value := meaningful(hit.group(0)))}


def argument_text(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, (int, float)):
        yield str(value)
    elif isinstance(value, dict):
        for child in value.values():
            yield from argument_text(child)
    elif isinstance(value, list):
        for child in value:
            yield from argument_text(child)


def simple_command(command):
    lexer = shlex.shlex(command, posix=True, punctuation_chars="|&;()<>")
    lexer.whitespace_split = True
    lexer.commenters = ""
    try:
        parts = list(lexer)
    except ValueError:
        return False
    return bool(parts) and not any(
        part and all(char in "|&;()<>" for char in part) for part in parts
    ) and not any(pattern in command for pattern in ("\n", "`", "$(", "${"))


def utf8_bounds(data, head, tail):
    while head > 0 and data[head] & 0b11000000 == 0b10000000:
        head -= 1
    while tail < len(data) and data[tail] & 0b11000000 == 0b10000000:
        tail += 1
    return head, tail


def removed_only(data, wanted, windows):
    found = [set() for _ in windows]
    kept = [set() for _ in windows]
    if not wanted or not windows:
        return found
    for match in TOKEN.finditer(data):
        term = meaningful(match.group(0))
        if term not in wanted:
            continue
        start, end = match.span()
        for idx, (head, tail) in enumerate(windows):
            if end <= head or start >= tail:
                kept[idx].add(term)
            elif start >= head and end <= tail:
                found[idx].add(term)
    return [mid - visible for mid, visible in zip(found, kept, strict=True)]


def diagnostics(data):
    offsets = []
    start = 0
    lines = data.split(b"\n")
    for line in lines:
        end = start + len(line)
        if DIAGNOSTIC.search(baseline.ANSI.sub(b"", line)):
            offsets.append((start, end))
        start = end + 1
    return offsets


@dataclass(frozen=True, slots=True)
class BashOutput:
    stdout: bytes
    parsed: dict | None
    clipped: bool
    legacy: bool

    def compact_result(self):
        return len(baseline.compact(self.parsed)) if self.parsed is not None else 0

    def capped_result(self, head, tail, seq):
        source = f"@self#seq-{seq}"
        omitted = tail - head
        marker = f"[{omitted} bytes omitted; read_conversation {source}]"
        compact = self.parsed.copy()
        compact["lines"] = {
            "head": self.stdout[:head].decode().split("\n"),
            "omission": marker,
            "tail": self.stdout[tail:].decode().split("\n"),
            "tail_start_offset": (compact.get("start_offset") or 0) + self.stdout.count(b"\n", 0, tail),
        }
        return len(baseline.compact(compact))


def parse_output(text, operation):
    clipped = bool(RUNTIME_TRUNCATION.search(text))
    obj = baseline.parse(text)
    lines = obj.get("lines") if isinstance(obj, dict) else None
    if not isinstance(lines, list):
        return BashOutput(text.encode(), None, clipped, False)
    legacy = any(isinstance(line, dict) for line in lines)
    decoded = [line.get("bytes") if isinstance(line, dict) else line for line in lines]
    if not all(isinstance(line, str) for line in decoded):
        return BashOutput(text.encode(), None, clipped, legacy)
    compact = obj.copy()
    compact["lines"] = decoded
    if operation == "run":
        compact.pop("cmd", None)
        compact.pop("command", None)
    return BashOutput("\n".join(decoded).encode(), compact, clipped, legacy)


@dataclass(slots=True)
class Transcript(ExactTranscript):
    bash: dict = field(default_factory=dict)
    commands: dict = field(default_factory=dict)
    assistant_content: dict = field(default_factory=dict)
    assistant_tool_args: dict = field(default_factory=dict)
    user_turns: list = field(default_factory=list)

    def add(self, row, hints, images):
        _, _, seq, kind, _, _, _ = row
        if kind in {"user", "continuation"}:
            self.user_turns.append(seq)
        ExactTranscript.add(self, row, hints, images)

    def add_assistant(self, seq, raw, data, usage, created, hints):
        ExactTranscript.add_assistant(self, seq, raw, data, usage, created, hints)
        text = []
        tool_args = []
        saw_tool = False
        for block in data if isinstance(data, list) else ():
            if not isinstance(block, dict):
                continue
            if block.get("type") == "text" and isinstance(block.get("text"), str):
                text.append(block["text"])
            if block.get("type") == "tool_use":
                saw_tool = True
                args = baseline.object_value(block.get("input"))
                tool_args.extend(argument_text(args))
                call = self.calls.get(block.get("id"))
                command = args.get("cmd") or args.get("command")
                if call is not None and call.tool == "bash" and isinstance(command, str):
                    self.commands[id(call)] = command
        self.assistant_content[seq] = tokens("\n".join(text).encode())
        if saw_tool:
            self.assistant_tool_args[seq] = tokens("\n".join(tool_args).encode())

    def add_result(self, mid, seq, body, images):
        ExactTranscript.add_result(self, mid, seq, body, images)
        item = self.results[-1]
        if item.call.tool == "bash" and item.size > 8 * KIB:
            text = body.get("content")
            if isinstance(text, str):
                self.bash[seq] = parse_output(text, self.operations.get(id(item.call), "unknown"))


def load_reader(db):
    initial = dict(db.execute("""
        SELECT c.conversation_id,
               EXISTS(SELECT 1 FROM conversation_tool_context_initial i
                      WHERE i.conversation_id=c.conversation_id AND i.name='read_conversation')
        FROM conversation_tool_contexts c
    """))
    latest = dict(db.execute("""
        SELECT conversation_id,max(name='read_conversation') FROM conversation_advertised_tools GROUP BY 1
    """))
    changes = defaultdict(list)
    for conv, seq, kind in db.execute("""
        SELECT c.conversation_id,m.sequence_id,c.kind FROM conversation_tool_context_changes c
        JOIN messages m ON m.message_id=c.after_message_id WHERE c.name='read_conversation'
        ORDER BY c.conversation_id,m.sequence_id,c.ordinal
    """):
        changes[conv].append((seq, kind == "addition"))
    return initial, changes, latest


def reader_at(seq, initial, changes):
    current = initial
    for position, available in changes:
        if position >= seq:
            break
        current = available
    return current


def update_diagnostics(stats, data, positions):
    stats["outputs"] += 1
    if not positions:
        return
    stats["outputs_with_diagnostics"] += 1
    stats["diagnostic_lines"] += len(positions)
    for start, _ in positions:
        stats[f"byte_decile_{min(10, 1 + 10 * start // len(data))}"] += 1
    front, back = 4 * KIB, len(data) - 12 * KIB
    stats["all_diagnostics_first_4k"] += all(end <= front for _, end in positions)
    stats["all_diagnostics_last_12k"] += all(start >= back for start, _ in positions)
    stats["all_diagnostics_in_head_or_tail"] += all(end <= front or start >= back for start, end in positions)


def price_cap(stats, output, item, current_bytes, head, tail, slope, positions, hit, strict,
              prose_hit, tool_hit, followup, request, reader):
    stats["capped_results"] += 1
    stats["with_assistant_or_call_followup"] += followup
    stats["lost_evidence"] += hit
    stats["lost_evidence_not_in_command"] += strict
    stats["lost_next_message_terms"] += prose_hit
    stats["lost_next_tool_argument_terms"] += tool_hit
    stats["with_diagnostics"] += bool(positions)
    stats["diagnostic_line_omitted_or_split"] += any(start < tail and end > head for start, end in positions)
    stats["all_diagnostics_omitted_or_split"] += bool(positions) and all(
        start < tail and end > head for start, end in positions
    )
    stats["full_carry_requests"] += item.carries
    stats["omitted_bytes"] += tail - head
    stats["reader_available"] += reader is True
    stats["reader_missing"] += reader is False
    stats["reader_unknown"] += reader is None
    stats["lost_evidence_without_known_reader"] += hit and reader is not True
    proposed = output.capped_result(head, tail, item.seq)
    removed = max(0, current_bytes - proposed)
    saved = removed * slope * item.carries
    retrieval = (tail - head) * slope if hit else 0.0
    stats["saved_before_retrieval"] += saved
    stats["one_slice_retrieval"] += retrieval
    stats["net_after_one_slice_retrieval"] += saved - retrieval
    stats["net_after_slice_and_extra_request"] += saved - retrieval - (
        max(0, request.total - removed * slope) if hit and request is not None else 0
    )
    if reader is True:
        stats["reader_available_net_after_one_slice_retrieval"] += saved - retrieval


@dataclass(slots=True)
class Audit:
    clearing: baseline.Reconstruction = field(default_factory=baseline.Reconstruction)
    counts: Counter = field(default_factory=Counter)
    diagnostics: dict = field(default_factory=lambda: defaultdict(Counter))
    by_family: dict = field(default_factory=lambda: defaultdict(Counter))

    def add(self, t, initial, edits):
        if not t.bash:
            return
        slope, _ = baseline.calibrate(t.requests)
        baseline.reconstruct(t, slope, self.clearing)
        request_seqs = [r.seq for r in t.requests]
        assistant_seqs = list(t.assistant_content)
        tool_seqs = list(t.assistant_tool_args)
        for item in t.results:
            output = t.bash.get(item.seq)
            if output is None:
                continue
            data = output.stdout
            if len(data) <= 8 * KIB:
                continue
            family = item.call.bash.family
            operation = t.operations.get(id(item.call), "unknown")
            self.counts[f"large_{operation}_seen"] += 1
            self.counts["already_runtime_clipped"] += output.clipped
            self.counts["unstructured"] += output.parsed is None
            self.counts["legacy_structured"] += output.legacy
            positions = diagnostics(data)
            if len(data) > 16 * KIB:
                for name in ("all", family):
                    update_diagnostics(self.diagnostics[name], data, positions)
                self.counts[f"over_16k_{operation}"] += 1
                self.counts["over_16k_unstructured"] += output.parsed is None
            user_idx = bisect_right(t.user_turns, item.seq)
            user_stop = t.user_turns[user_idx] if user_idx < len(t.user_turns) else float("inf")
            prose_idx = bisect_right(assistant_seqs, item.seq)
            call_idx = bisect_right(tool_seqs, item.seq)
            prose = (t.assistant_content[assistant_seqs[prose_idx]]
                     if prose_idx < len(assistant_seqs) and assistant_seqs[prose_idx] < user_stop else set())
            has_prose = prose_idx < len(assistant_seqs) and assistant_seqs[prose_idx] < user_stop
            has_call = call_idx < len(tool_seqs) and tool_seqs[call_idx] < user_stop
            calls = t.assistant_tool_args[tool_seqs[call_idx]] if has_call else set()
            followup = has_prose or has_call
            request_idx = bisect_right(request_seqs, item.seq)
            request = t.requests[request_idx] if request_idx < len(t.requests) else None
            reader = reader_at(request.seq, initial, edits) if request is not None else None
            command = t.commands.get(id(item.call), "")
            command_tokens = tokens(command.encode())
            active_caps = [(cap, utf8_bounds(data, head * KIB, len(data) - (cap - head) * KIB))
                           for cap, head in CAPS if len(data) > cap * KIB]
            evidence = removed_only(data, prose | calls, [limits for _, limits in active_caps])
            for (cap, (head, tail)), removed_terms in zip(active_caps, evidence, strict=True):
                if len(data) > 16 * KIB:
                    stats = self.by_family["evidence_all_bash_over_16k", cap, family]
                    stats["outputs"] += 1
                    stats["with_followup"] += followup
                    stats["lost_evidence"] += bool(removed_terms)
                    stats["lost_evidence_not_in_command"] += bool(removed_terms - command_tokens)
                    stats["lost_next_tool_argument_terms"] += bool(removed_terms & calls)
                if operation != "run" or output.parsed is None or output.clipped:
                    continue
                if not item.carries:
                    self.counts[f"cap_{cap}k_zero_full_carries"] += 1
                    continue
                current_bytes = output.compact_result()
                keys = [("structured_run", cap, family)]
                if not output.legacy:
                    keys.append(("compact_wire_run", cap, family))
                success = (not item.error and output.parsed.get("status") == "exited"
                           and output.parsed.get("exit_code") == 0)
                if success and family in SAFE_BUILD_FAMILIES and command and simple_command(command):
                    keys.append(("single_successful_build", cap, family))
                    if reader is True and not any(start < tail and end > head for start, end in positions):
                        keys.append(("successful_build_diagnostics_preserved_with_reader", cap, family))
                if len(data) > 16 * KIB:
                    keys.append(("structured_run_over_16k", cap, family))
                for key in keys:
                    price_cap(self.by_family[key], output, item, current_bytes, head, tail, slope, positions,
                              bool(removed_terms), bool(removed_terms - command_tokens), bool(removed_terms & prose),
                              bool(removed_terms & calls), followup, request, reader)

    def report(self, recorded_input):
        nested = defaultdict(lambda: defaultdict(dict))
        for (cohort, cap, family), stats in sorted(self.by_family.items()):
            nested[cohort][f"{cap}k"][family] = dict(stats)
        return {"recorded_input": recorded_input, "counts": dict(self.counts),
                "diagnostics_by_family": dict(self.diagnostics), "caps": dict(nested)}


def analyze(db):
    recorded = db.execute("SELECT coalesce(sum(input_tokens+cache_creation_tokens+cache_read_tokens),0) FROM turn_usage").fetchone()[0]
    conversations = {cid: (role, watermark) for cid, role, watermark in db.execute(
        "SELECT id,runtime_role,coalesce(clear_watermark,0) FROM conversations"
    )}
    hints = defaultdict(lambda: defaultdict(list))
    for conv, created, reduced, *usage in db.execute("""
        SELECT u.conversation_id,u.created_at,p.reduced_local_tokens,u.input_tokens,
               u.cache_creation_tokens,u.cache_read_tokens,u.output_tokens
        FROM turn_usage u JOIN request_capacity_plans p ON p.plan_id=u.capacity_plan_id
        WHERE p.kind='ordinary' AND p.reduction_action IN ('none','clearing')
    """):
        hints[conv][tuple(usage)].append((baseline.timestamp(created), reduced))
    images = Counter(mid for (mid,) in db.execute("SELECT message_id FROM message_images"))
    reader, changes, latest = load_reader(db)
    audit = Audit()
    sql = """SELECT conversation_id,message_id,sequence_id,message_type,content,usage_data,created_at
             FROM messages ORDER BY conversation_id,sequence_id"""
    for index, (conv, rows) in enumerate(groupby(db.execute(sql), key=lambda row: row[0]), 1):
        if conv not in conversations:
            continue
        role, watermark = conversations[conv]
        t = Transcript(role, watermark, conversation_id=conv)
        for row in rows:
            t.add(row, hints.get(conv, {}), images)
        initial = bool(reader[conv]) if conv in reader else bool(latest[conv]) if conv in latest else None
        audit.add(t, initial, changes.get(conv, ()))
        if index % 2500 == 0:
            print(f"Replayed {index} transcripts.", file=sys.stderr, flush=True)
    return audit.report(recorded)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("database", type=Path)
    parser.add_argument("--json", required=True, type=Path, help="write a new aggregate-only file")
    args = parser.parse_args()
    if args.json.exists():
        parser.error("select a new JSON destination")
    with closing(baseline.open_read_only(args.database.expanduser())) as db:
        result = analyze(db)
    with args.json.open("x", encoding="utf-8") as output:
        json.dump(result, output, indent=2, allow_nan=False)
        output.write("\n")


if __name__ == "__main__":
    main()
