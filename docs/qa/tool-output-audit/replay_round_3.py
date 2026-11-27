#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.13"
# dependencies = []
# ///
"""Replay the exact Round 3 clearable-preview and marker formats."""

from __future__ import annotations

import argparse
from bisect import bisect_left
from collections import Counter, defaultdict
from contextlib import closing
from dataclasses import dataclass, field
from itertools import groupby
import json
from pathlib import Path
import re
import sqlite3
import sys

from replay_round_2 import (
    ReplayTranscript, baseline, old_marker, recover_sweeps,
)


OUTPUT_CAP = 100 * 1024
RUNTIME_TRUNCATION = re.compile(r"\n…\[truncated \d+ bytes of \d+ total\]…\n")
BASH_CUTOFF = "2026-11-24"


def local_estimate(characters: int, images: int = 0) -> int:
    """Phoenix's estimator counts Unicode scalar values, then images."""
    return (characters + 3) // 4 + images * baseline.IMAGE_TOKENS


def revised_marker(seq: int) -> str:
    return f"[tool result cleared; read_conversation @self#seq-{seq}]"


def rust_lines(text: str):
    """Rust str::lines recognizes LF and CRLF, not every Unicode line separator."""
    for line in text.split("\n"):
        yield line.removesuffix("\r")


def exact_patch_replacement(content: str, marker: str) -> tuple[str, str]:
    if len(content.encode()) > OUTPUT_CAP or RUNTIME_TRUNCATION.search(content):
        return "runtime_cap", ""
    head, opening, rest = content.partition("<diff>\n")
    preview, closing_tag, tail = rest.partition("</diff>")
    if not opening or not closing_tag or "<patches_applied>all</patches_applied>\n" not in head:
        return "no_preview", ""
    summary = "\n".join(line for line in rust_lines(preview) if line.startswith("@@ "))
    replacement = f"{summary}\n{marker}" if summary else marker
    return "preview", head + replacement + tail


@dataclass(frozen=True, slots=True)
class Costs:
    full_local: int
    old_marker_bytes: int
    old_marker_local: int
    revised_marker_bytes: int
    revised_marker_local: int
    patch_shape: str
    patch_cleared_bytes: int
    patch_cleared_local: int


@dataclass(slots=True)
class ExactTranscript(ReplayTranscript):
    costs: dict[int, Costs] = field(default_factory=dict)
    operations: dict[int, str] = field(default_factory=dict)
    bash_echo: dict[int, tuple[str, str, int]] = field(default_factory=dict)
    current_created_at: str = ""

    def add(self, row, hints, image_counts):
        self.current_created_at = row[-1]
        baseline.Transcript.add(self, row, hints, image_counts)

    def add_assistant(self, seq, raw, data, usage, created_at, hints):
        baseline.Transcript.add_assistant(self, seq, raw, data, usage, created_at, hints)
        self.assistant.clear()
        for raw_block in data if isinstance(data, list) else ():
            block = baseline.object_value(raw_block)
            if block.get("type") != "tool_use" or block.get("name") != "bash":
                continue
            call = self.calls[block.get("id", "")]
            args = baseline.object_value(block.get("input"))
            op = args.get("op")
            if op not in ("run", "peek", "wait", "kill"):
                op = "run" if args.get("cmd") or args.get("command") else "unknown"
            self.operations[id(call)] = op

    def add_result(self, mid, seq, body, image_counts):
        ReplayTranscript.add_result(self, mid, seq, body, image_counts)
        item = self.results[-1]
        content = body.get("content") or ""
        if not isinstance(content, str):
            content = baseline.compact(content).decode()
        old = old_marker(self.conversation_id, mid, seq).decode()
        new = revised_marker(seq)
        patch_shape = "none"
        patch_cleared = ""
        if item.call.tool == "patch":
            if item.error:
                patch_shape = "error"
            else:
                patch_shape, patch_cleared = exact_patch_replacement(content, old)
        self.costs[seq] = Costs(
            local_estimate(len(content), item.images), len(old.encode()), local_estimate(len(old)),
            len(new.encode()), local_estimate(len(new)), patch_shape, len(patch_cleared.encode()),
            local_estimate(len(patch_cleared)),
        )
        if item.call.tool == "bash":
            structured = baseline.object_value(baseline.parse(content))
            cmd = structured.get("cmd")
            if not isinstance(cmd, str) or not cmd:
                cmd = structured.get("command")
            echo_bytes = len(baseline.compact(cmd)) + len(',"cmd":') if isinstance(cmd, str) and cmd else 0
            date = self.current_created_at[:10]
            epoch = "before" if date < BASH_CUTOFF else "on_or_after"
            self.bash_echo[seq] = (epoch, self.operations.get(id(item.call), "unknown"), echo_bytes)


@dataclass(slots=True)
class ExactReplay:
    corpus: Counter = field(default_factory=Counter)
    clearing: baseline.Reconstruction = field(default_factory=baseline.Reconstruction)
    patches: Counter = field(default_factory=Counter)
    patch_gain_bytes: list[int] = field(default_factory=list)
    b_by_shape: dict[str, Counter] = field(default_factory=lambda: defaultdict(Counter))
    compact: dict[str, Counter] = field(default_factory=lambda: defaultdict(Counter))
    growth: dict[str, Counter] = field(default_factory=lambda: defaultdict(Counter))
    bash: dict[str, dict[str, Counter]] = field(
        default_factory=lambda: defaultdict(lambda: defaultdict(Counter)))

    def add(self, transcript: ExactTranscript):
        slope, _ = baseline.calibrate(transcript.requests)
        baseline.reconstruct(transcript, slope, self.clearing)
        self.corpus.update({"transcripts": 1, "response_anchors": len(transcript.requests)})
        sweeps, positions = recover_sweeps(transcript)
        cutoffs = [sweep.cutoff for sweep in sweeps]
        for item in transcript.results:
            costs = transcript.costs[item.seq]
            if item.seq in positions:
                turns = item.upper_carries - item.carries
                self.marker("whole_outputs", turns, costs, slope)
                self.growing("current", item, costs.full_local, costs.old_marker_local,
                             costs.old_marker_bytes, turns, slope)
                self.growing("revised", item, costs.full_local, costs.revised_marker_local,
                             costs.revised_marker_bytes, turns, slope)
            if item.call.tool == "patch":
                i = bisect_left(cutoffs, item.seq)
                counterfactual_turns = len(transcript.requests) - sweeps[i].index if i < len(sweeps) else 0
                self.patch(item, costs, counterfactual_turns, slope)
            if item.call.tool == "bash":
                epoch, operation, echo_bytes = transcript.bash_echo[item.seq]
                for label in ("all", operation):
                    self.bash[epoch][label].update({"outputs": 1, "with_command_echo": echo_bytes > 0,
                                                   "echo_carry": echo_bytes * item.carries * slope,
                                                   "echo_upper": echo_bytes * item.upper_carries * slope})

    def marker(self, label: str, turns: int, costs: Costs, slope: float):
        self.compact[label].update({
            "result_rows": 1, "marker_turns": turns,
            "current_carry": costs.old_marker_bytes * turns * slope,
            "revised_carry": costs.revised_marker_bytes * turns * slope,
            "current_local_carry": costs.old_marker_local * turns,
            "revised_local_carry": costs.revised_marker_local * turns,
        })

    def growing(self, label, item, full_local, marker_local, marker_bytes, turns, slope):
        if full_local > marker_local:
            return
        group = self.growth[label]
        gain = marker_local - full_local
        group.update({"rows_at_or_below_marker": 1, "rows_that_grow": gain > 0,
                      "marker_turns": turns, "local_tokens_added": gain * turns})
        calibrated_delta = (marker_bytes - item.size) * slope - item.images * baseline.IMAGE_TOKENS
        group["calibrated_extra_for_growing_rows"] += max(0, calibrated_delta) * turns if gain else 0

    def patch(self, item: baseline.Result, costs: Costs, turns: int, slope: float):
        upper_carry = (item.size * slope + item.images * baseline.IMAGE_TOKENS) * item.upper_carries
        self.patches.update({"results": 1, "unmodified_carry": upper_carry})
        shape = costs.patch_shape
        if shape == "preview" and costs.patch_cleared_local >= costs.full_local:
            shape = "shrink_gate"
        self.b_by_shape[shape].update({"results": 1, "unmodified_carry": upper_carry,
                                       "crossed_sweep": turns > 0})
        if shape == "shrink_gate":
            extra = max(0, costs.patch_cleared_local - costs.full_local)
            self.b_by_shape[shape]["local_added_avoided"] += extra * turns
            calibrated = (costs.patch_cleared_bytes - item.size) * slope - item.images * baseline.IMAGE_TOKENS
            self.b_by_shape[shape]["calibrated_added_avoided"] += max(0.0, calibrated) * turns
        if shape == "preview" and turns > 0:
            gain = item.size - costs.patch_cleared_bytes
            self.patches.update({"previews_cleared": 1,
                                 "calibrated_savings": (gain * slope + item.images * baseline.IMAGE_TOKENS) * turns,
                                 "local_savings": (costs.full_local - costs.patch_cleared_local) * turns})
            self.patch_gain_bytes.append(gain)
            self.marker("patch_previews", turns, costs, slope)

    def report(self, ledger, ledger_epochs, partial):
        b = dict(self.patches)
        b["shape_groups"] = dict(self.b_by_shape)
        b["calibrated_savings_share"] = baseline.ratio(b.get("calibrated_savings", 0), ledger)
        b["local_savings_share"] = baseline.ratio(b.get("local_savings", 0), ledger)
        b["net_bytes_per_cleared_preview"] = baseline.quantiles(self.patch_gain_bytes)
        total = Counter()
        for stats in self.compact.values():
            total.update(stats)
        a = dict(total)
        a["by_carrier"] = dict(self.compact)
        a["calibrated_savings"] = a.get("current_carry", 0) - a.get("revised_carry", 0)
        a["calibrated_savings_share"] = baseline.ratio(a["calibrated_savings"], ledger)
        a["local_savings"] = a.get("current_local_carry", 0) - a.get("revised_local_carry", 0)
        a["local_savings_share"] = baseline.ratio(a["local_savings"], ledger)
        a["foreign_clearances_in_grouped_replay"] = 0
        growth = {}
        for label, stats in self.growth.items():
            growth[label] = {**stats, "share_of_recorded_input": baseline.ratio(
                stats["calibrated_extra_for_growing_rows"], ledger)}
        return {"partial_run": partial, "recorded_input": ledger, "corpus": dict(self.corpus),
                "candidate_b": b, "revised_a_after_b": a, "growing_clearances": growth,
                "bash_echo_by_epoch": dict(self.bash), "recorded_input_by_epoch": ledger_epochs}


def analyze(db: sqlite3.Connection, limit: int | None) -> dict:
    ledger_epochs = dict(db.execute(
        """SELECT CASE WHEN substr(created_at,1,10) < ? THEN 'before' ELSE 'on_or_after' END AS epoch,
                  coalesce(sum(input_tokens + cache_creation_tokens + cache_read_tokens), 0)
           FROM turn_usage GROUP BY epoch""", (BASH_CUTOFF,)
    ))
    ledger = sum(ledger_epochs.values())
    conversations = {row[0]: row[1:] for row in db.execute(
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
    replay = ExactReplay()
    for index, (conv, group) in enumerate(groupby(rows, key=lambda row: row[0])):
        if limit is not None and index >= limit:
            break
        if conv not in conversations:
            continue
        transcript = ExactTranscript(*conversations[conv], conversation_id=conv)
        hints = anchors.get(conv, {})
        for row in group:
            transcript.add(row, hints, images)
        replay.add(transcript)
        if (index + 1) % 2500 == 0:
            print(f"Replayed {index + 1} transcripts.", file=sys.stderr)
    return replay.report(ledger, ledger_epochs, limit is not None)


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
