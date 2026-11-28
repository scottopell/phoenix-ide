#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.13"
# dependencies = []
# ///
"""Replay the two Round 2 changes against the Round 1 clearing schedule."""

from __future__ import annotations

import argparse
from bisect import bisect_left, bisect_right
from collections import Counter, defaultdict
from contextlib import closing
from dataclasses import dataclass, field
from itertools import groupby
import json
from pathlib import Path
import re
import sqlite3
import sys
from urllib.parse import unquote

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "scripts"))
import tool_output_audit as baseline

RETRIEVAL_TOOLS = frozenset({
    "read_conversation", "query_database", "previous_transcripts",
    "read_event_evidence", "resolve_reference",
})
HUNK = re.compile(rb"^@@ -\d+(?:,\d+)? \+\d+(?:,\d+)? @@")
REFERENCE_WORDS = re.compile(r"[a-zA-Z0-9_:-]+")
MESSAGE_FRAGMENT = re.compile(r"#message-([a-zA-Z0-9_:-]+)")
SEQUENCE_PREDICATE = re.compile(r"\bsequence_id\s*=\s*(\d+)\b")
LANDMARK_AGES = (10, 25, 50, 100)
FOLLOW_UP = 20


def old_marker(conversation_id: str, message_id: str, sequence: int) -> bytes:
    return (f"[tool result cleared to save context] [persisted source: "
            f"conversation={conversation_id}, message={message_id}, sequence={sequence}]").encode()


def compact_marker(sequence: int) -> bytes:
    return f"[tool result cleared; seq={sequence}]".encode()


def patch_preview_marker(sequence: int) -> bytes:
    return f"[patch preview cleared; seq={sequence}]".encode()


def local_tokens(byte_count: int) -> int:
    return (byte_count + 3) // 4


@dataclass(frozen=True, slots=True)
class Preview:
    record_size: int
    original_size: int
    removed_bytes: int
    marker_size: int
    hunk_headers: int
    hunk_suffixes: int
    has_omission_note: bool


def split_preview(data: bytes, seq: int) -> Preview | None:
    before, opening, tail = data.partition(b"<diff>")
    diff, closing_tag, after = tail.partition(b"</diff>")
    if not opening or not closing_tag or b"<patches_applied>all</patches_applied>" not in before:
        return None
    header_bytes = hunk_headers = hunk_suffixes = 0
    has_omission_note = False
    for line in diff.split(b"\n"):
        if match := HUNK.match(line):
            header_bytes += match.end() + 1
            hunk_headers += 1
            hunk_suffixes += bool(line[match.end():].strip())
        if line.startswith(b"... diff truncated:"):
            header_bytes += len(line) + 1
            has_omission_note = True
    # Preserve the existing output outside the preview, including any warning
    # and next-step metadata. The input retains the exact path. Retain the
    # omission note because the stored preview may already be truncated.
    record_size = len(before) + len(opening) + 1 + header_bytes + len(closing_tag) + len(after)
    marker_size = len(patch_preview_marker(seq)) + 1
    removed_bytes = max(0, len(data) - record_size)
    return Preview(record_size, len(data), removed_bytes, marker_size, hunk_headers,
                   hunk_suffixes, has_omission_note)


@dataclass(slots=True)
class ReplayTranscript(baseline.Transcript):
    conversation_id: str = ""
    message_ids: dict[int, str] = field(default_factory=dict)
    previews: dict[int, Preview] = field(default_factory=dict)
    retrieval_calls: list[tuple[int, str, str]] = field(default_factory=list)
    path_reads: dict[bytes, list[int]] = field(default_factory=lambda: defaultdict(list))
    git_reads: list[int] = field(default_factory=list)
    patch_failures: dict[bytes, list[int]] = field(default_factory=lambda: defaultdict(list))
    patch_path_sizes: dict[bytes, int] = field(default_factory=dict)

    def add_assistant(self, seq, raw, data, usage, created_at, hints):
        baseline.Transcript.add_assistant(self, seq, raw, data, usage, created_at, hints)
        self.assistant.clear()
        for source in data if isinstance(data, list) else ():
            block = baseline.object_value(source)
            if block.get("type") != "tool_use":
                continue
            call = self.calls[block.get("id", "")]
            if call.tool == "read_file" and call.path:
                self.path_reads[call.path].append(seq)
            if call.tool == "patch" and call.path:
                path = baseline.object_value(block.get("input")).get("path")
                self.patch_path_sizes[call.path] = len(baseline.compact(path)) + len("\n<path></path>")
            if call.tool == "bash" and call.bash.family in {"git show", "git diff"}:
                self.git_reads.append(seq)
            if call.tool in RETRIEVAL_TOOLS:
                arguments = baseline.compact(block.get("input")).decode()
                self.retrieval_calls.append((seq, call.tool, arguments))

    def add_result(self, mid, seq, body, image_counts):
        call = self.calls.get(body.get("tool_use_id"))
        if call is None:
            call = baseline.Call("unpaired_tool", b"", 0, None, None)
        content = body.get("content") or ""
        if not isinstance(content, str):
            content = baseline.compact(content).decode()
        data = content.encode()
        error = bool(body.get("is_error"))
        images = max(len(body.get("images") or []), image_counts.get(mid, 0))
        if call.tool == "bash":
            shape = baseline.object_value(baseline.parse(content))
            handle = shape.get("handle")
            if isinstance(handle, str) and handle:
                self.handles[handle] = call.bash
        if call.tool == "patch":
            if error and call.path:
                self.patch_failures[call.path].append(seq)
            elif not error and (preview := split_preview(data, seq)):
                self.previews[seq] = preview
        clearable = call.tool in baseline.CLEARABLE or call.tool == "skill" and self.role == "coordinator"
        marker = 113 + len(mid) + len(str(seq))
        item = baseline.Result(seq, max(0, self.call_round), call, clearable, len(data),
                               marker, images, error, {}, 0, 0, ())
        self.results.append(item)
        self.message_ids[seq] = mid
        self.result_round = max(self.result_round, item.round)
        self.input_bytes += item.size
        self.input_images += item.images


@dataclass(frozen=True, slots=True)
class Sweep:
    index: int
    cutoff: int
    gain: int


def recover_sweeps(transcript: ReplayTranscript) -> tuple[list[Sweep], dict[int, int]]:
    request_count = len(transcript.requests)
    positions = {}
    groups = defaultdict(list)
    for item in transcript.results:
        marker_turns = item.upper_carries - item.carries
        if marker_turns > 0:
            index = request_count - marker_turns
            positions[item.seq] = index
            groups[index].append(item)
    sweeps = []
    for index, items in sorted(groups.items()):
        gain = sum(local_tokens(item.size) - local_tokens(item.marker_size) +
                   baseline.IMAGE_TOKENS * item.images for item in items)
        sweeps.append(Sweep(index, max(item.seq for item in items), gain))
    return sweeps, positions


def next_action(seqs: list[int], lower: int, upper: int) -> bool:
    index = bisect_left(seqs, lower)
    return index < len(seqs) and seqs[index] < upper


def risk_flags(transcript: ReplayTranscript, path: bytes | None, lower: int, upper: int) -> tuple[bool, ...]:
    return (
        bool(path and next_action(transcript.path_reads.get(path, []), lower, upper)),
        next_action(transcript.git_reads, lower, upper),
        bool(path and next_action(transcript.patch_failures.get(path, []), lower, upper)),
    )


def update_risk(counter: Counter, flags: tuple[bool, ...]):
    counter["patches"] += 1
    for name, flag in zip(("same_path_read", "git_diff_or_show", "same_path_failed_patch"), flags):
        counter[name] += flag


@dataclass(slots=True)
class Replay:
    corpus: Counter = field(default_factory=Counter)
    clearing: baseline.Reconstruction = field(default_factory=baseline.Reconstruction)
    markers: Counter = field(default_factory=Counter)
    marker_bytes: list[int] = field(default_factory=list)
    compact_bytes: list[int] = field(default_factory=list)
    marker_tokens: list[float] = field(default_factory=list)
    compact_tokens: list[float] = field(default_factory=list)
    retrieval: dict[str, Counter] = field(default_factory=lambda: defaultdict(Counter))
    patches: Counter = field(default_factory=Counter)
    record_tokens: list[float] = field(default_factory=list)
    self_contained_record_tokens: list[float] = field(default_factory=list)
    gain_all: list[int] = field(default_factory=list)
    gain_added: list[int] = field(default_factory=list)
    treated: Counter = field(default_factory=Counter)
    strata: dict[tuple, dict[str, Counter]] = field(
        default_factory=lambda: defaultdict(lambda: defaultdict(Counter)))

    def add(self, transcript: ReplayTranscript):
        slope, _ = baseline.calibrate(transcript.requests)
        baseline.reconstruct(transcript, slope, self.clearing)
        self.corpus["transcripts"] += 1
        self.corpus["request_anchors"] += len(transcript.requests)
        self.corpus["anchored_input"] += sum(req.total for req in transcript.requests)
        sweeps, positions = recover_sweeps(transcript)
        self.add_markers(transcript, positions, slope)
        self.add_patches(transcript, sweeps, slope)

    def add_markers(self, transcript: ReplayTranscript, positions: dict[int, int], slope: float):
        requests = transcript.requests
        if not positions:
            return
        self.markers["transcripts_with_markers"] += 1
        self.markers["requests_with_markers"] += len(requests) - min(positions.values())
        lookup = {transcript.message_ids[seq]: requests[index].seq for seq, index in positions.items()}
        by_seq = {seq: requests[index].seq for seq, index in positions.items()}
        for seq, index in positions.items():
            current = old_marker(transcript.conversation_id, transcript.message_ids[seq], seq)
            proposed = compact_marker(seq)
            turns = len(requests) - index
            self.marker_bytes.append(len(current))
            self.compact_bytes.append(len(proposed))
            self.marker_tokens.append(len(current) * slope)
            self.compact_tokens.append(len(proposed) * slope)
            self.markers.update({"result_rows": 1, "marker_turns": turns,
                                 "current_calibrated": len(current) * slope * turns,
                                 "proposed_calibrated": len(proposed) * slope * turns,
                                 "current_bytes_quarter": local_tokens(len(current)) * turns,
                                 "proposed_bytes_quarter": local_tokens(len(proposed)) * turns})
        retrieved = set()
        directly_retrieved = set()
        first = requests[min(positions.values())].seq
        for seq, tool, args in transcript.retrieval_calls:
            if seq < first:
                continue
            self.retrieval[tool]["later_calls"] += 1
            retrieved.add(tool)
            text = unquote(args)
            explicit_ids = set(REFERENCE_WORDS.findall(text))
            explicit_ids.update(MESSAGE_FRAGMENT.findall(text))
            direct = any(lookup.get(identifier, seq + 1) <= seq for identifier in explicit_ids)
            if not direct and transcript.conversation_id in text:
                direct = any(by_seq.get(int(match[1]), seq + 1) <= seq
                             for match in SEQUENCE_PREDICATE.finditer(text))
            if direct:
                self.retrieval[tool]["exact_cleared_source_calls"] += 1
                directly_retrieved.add(tool)
        self.markers["transcripts_with_later_retrieval"] += bool(retrieved)
        self.markers["transcripts_with_exact_retrieval"] += bool(directly_retrieved)
        for tool in retrieved:
            self.retrieval[tool]["transcripts_with_later_call"] += 1
        for tool in directly_retrieved:
            self.retrieval[tool]["transcripts_with_exact_call"] += 1

    def add_patches(self, transcript: ReplayTranscript, sweeps: list[Sweep], slope: float):
        seqs = [request.seq for request in transcript.requests]
        cutoffs = [sweep.cutoff for sweep in sweeps]
        extra = Counter()
        for item in transcript.results:
            if item.call.tool != "patch":
                continue
            turns = item.upper_carries
            self.patches.update({"results": 1, "unmodified_carry": item.size * slope * turns,
                                 "errors": item.error, "error_carry": item.size * slope * turns if item.error else 0})
            preview = transcript.previews.get(item.seq)
            if not preview:
                continue
            self.patches.update({"recognized_success_previews": 1, "hunks": preview.hunk_headers,
                                 "hunks_with_context_label": preview.hunk_suffixes,
                                 "pretruncated_previews": preview.has_omission_note})
            self.record_tokens.append(preview.record_size * slope)
            # A separate explicit path field duplicates the existing tool input.
            # Use the exact submitted path size plus a fixed field wrapper.
            explicit_path_bytes = transcript.patch_path_sizes.get(item.call.path, 0)
            self.self_contained_record_tokens.append((preview.record_size + explicit_path_bytes) * slope)
            self.patches["explicit_path_overhead"] += explicit_path_bytes * slope * turns
            start = bisect_right(seqs, item.seq)
            sweep_pos = bisect_left(cutoffs, item.seq)
            clear = sweeps[sweep_pos].index if sweep_pos < len(sweeps) else len(seqs)
            if clear < len(seqs) and preview.removed_bytes > preview.marker_size:
                cleared_turns = len(seqs) - clear
                gain_bytes = preview.removed_bytes - preview.marker_size
                self.patches.update({"cleared_previews": 1, "preview_marker_turns": cleared_turns,
                                     "carry_saved": gain_bytes * slope * cleared_turns,
                                     "preview_tokens_before_removal": preview.removed_bytes * slope * cleared_turns})
                extra[sweep_pos] += local_tokens(item.size) - local_tokens(item.size - gain_bytes)
                end = clear + FOLLOW_UP
                if end <= len(seqs):
                    update_risk(self.treated, risk_flags(transcript, item.call.path, seqs[clear],
                                                       seqs[end] if end < len(seqs) else sys.maxsize))
            for age in LANDMARK_AGES:
                mark = start + age
                end = mark + FOLLOW_UP
                if end > len(seqs):
                    continue
                if clear <= mark and preview.removed_bytes > preview.marker_size:
                    group = "cleared"
                elif clear >= end or preview.removed_bytes <= preview.marker_size:
                    group = "retained"
                else:
                    continue
                size_band = (preview.removed_bytes // 256).bit_length()
                history_band = (start // 32).bit_length()
                key = (age, transcript.role, size_band, history_band)
                update_risk(self.strata[key][group],
                            risk_flags(transcript, item.call.path, seqs[mark],
                                       seqs[end] if end < len(seqs) else sys.maxsize))
        for i, sweep in enumerate(sweeps):
            self.gain_all.append(sweep.gain)
            if added := extra[i]:
                self.gain_added.append(added)

    def risk_report(self) -> dict:
        grouped = defaultdict(lambda: defaultdict(Counter))
        for (age, *_), strata in self.strata.items():
            for group, stats in strata.items():
                grouped[age][group].update(stats)
            cleared = strata.get("cleared", {})
            retained = strata.get("retained", {})
            if cleared.get("patches") and retained.get("patches"):
                grouped[age]["cleared_with_retained_comparators"].update(cleared)
                denominator = retained["patches"]
                treated_count = cleared["patches"]
                weighted = {"patches": treated_count}
                for action in ("same_path_read", "git_diff_or_show", "same_path_failed_patch"):
                    weighted[action] = retained.get(action, 0) * treated_count / denominator
                grouped[age]["retained_standardized_to_cleared"].update(weighted)
        return {str(age): {group: dict(counts) for group, counts in grouped[age].items()}
                for age in sorted(grouped)}

    def report(self, totals, limited):
        ledger = sum(totals[1:])
        a = dict(self.markers)
        a["calibrated_savings"] = a.get("current_calibrated", 0) - a.get("proposed_calibrated", 0)
        a["bytes_quarter_savings"] = a.get("current_bytes_quarter", 0) - a.get("proposed_bytes_quarter", 0)
        a["calibrated_savings_share"] = baseline.ratio(a["calibrated_savings"], ledger)
        a["bytes_quarter_savings_share"] = baseline.ratio(a["bytes_quarter_savings"], ledger)
        a["foreign_source_rows_in_replay"] = 0
        a["current_marker_byte_distribution"] = baseline.quantiles(self.marker_bytes)
        a["compact_marker_byte_distribution"] = baseline.quantiles(self.compact_bytes)
        a["current_marker_token_distribution"] = baseline.quantiles(self.marker_tokens)
        a["compact_marker_token_distribution"] = baseline.quantiles(self.compact_tokens)
        a["retrieval_calls_by_tool"] = dict(self.retrieval)
        b = dict(self.patches)
        b["replay_carry"] = b.get("unmodified_carry", 0) - b.get("carry_saved", 0)
        b["saving_share"] = baseline.ratio(b.get("carry_saved", 0), ledger)
        b["carry_saved_with_explicit_path"] = b.get("carry_saved", 0) - b.get("explicit_path_overhead", 0)
        b["saving_share_with_explicit_path"] = baseline.ratio(b["carry_saved_with_explicit_path"], ledger)
        b["sweep_count"] = len(self.gain_all)
        b["sweeps_with_added_gain"] = len(self.gain_added)
        b["sweep_local_gain_before"] = sum(self.gain_all)
        b["sweep_local_gain_added"] = sum(self.gain_added)
        b["gain_added_distribution_on_affected_sweeps"] = baseline.quantiles(self.gain_added)
        b["record_tokens_using_path_in_call"] = baseline.quantiles(self.record_tokens)
        b["record_tokens_with_explicit_path_field"] = baseline.quantiles(self.self_contained_record_tokens)
        b["follow_up_measured_requests"] = FOLLOW_UP
        b["risk_immediately_after_clear"] = dict(self.treated)
        b["risk_at_fixed_age"] = self.risk_report()
        return {"partial_run": limited, "ledger": {"rows": totals[0], "recorded_input": ledger},
                "corpus": dict(self.corpus), "candidate_a": a, "candidate_b": b}


def analyze(db: sqlite3.Connection, limit: int | None) -> dict:
    totals = db.execute(
        """SELECT count(*), coalesce(sum(input_tokens),0), coalesce(sum(cache_creation_tokens),0),
                  coalesce(sum(cache_read_tokens),0) FROM turn_usage"""
    ).fetchone()
    convs = {row[0]: row[1:] for row in db.execute(
        "SELECT id, runtime_role, coalesce(clear_watermark,0) FROM conversations"
    )}
    images = Counter(row[0] for row in db.execute("SELECT message_id FROM message_images"))
    anchors = defaultdict(lambda: defaultdict(list))
    for conv, created, reduction, fresh, write, read, output in db.execute(
        """SELECT u.conversation_id, u.created_at, p.reduced_local_tokens,
                    u.input_tokens, u.cache_creation_tokens, u.cache_read_tokens, u.output_tokens
           FROM turn_usage u JOIN request_capacity_plans p ON u.capacity_plan_id=p.plan_id
           WHERE p.kind='ordinary' AND p.reduction_action IN ('none','clearing')"""
    ):
        anchors[conv][(fresh, write, read, output)].append((baseline.timestamp(created), reduction))
    rows = db.execute(
        """SELECT conversation_id, message_id, sequence_id, message_type, content, usage_data, created_at
           FROM messages ORDER BY conversation_id, sequence_id"""
    )
    replay = Replay()
    for index, (conv, group) in enumerate(groupby(rows, key=lambda row: row[0])):
        if limit is not None and index >= limit:
            break
        if conv not in convs:
            continue
        transcript = ReplayTranscript(*convs[conv], conversation_id=conv)
        hints = anchors.get(conv, {})
        for row in group:
            transcript.add(row, hints, images)
        replay.add(transcript)
        if (index + 1) % 2500 == 0:
            print(f"Replayed {index + 1} transcripts.", file=sys.stderr)
    return replay.report(totals, limit is not None)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("database", type=Path)
    parser.add_argument("--json", required=True, type=Path, help="write a new aggregate-only file")
    parser.add_argument("--limit-transcripts", type=int, help="partial validation run")
    args = parser.parse_args()
    if args.limit_transcripts is not None and args.limit_transcripts < 1:
        parser.error("the transcript limit must be positive")
    if args.json.exists():
        parser.error("select a new JSON destination")
    with closing(baseline.open_read_only(args.database.expanduser())) as db:
        result = analyze(db, args.limit_transcripts)
    with args.json.open("x", encoding="utf-8") as output:
        output.write(json.dumps(result, indent=2, allow_nan=False) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
