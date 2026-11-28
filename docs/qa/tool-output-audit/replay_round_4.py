#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.13"
# dependencies = []
# ///
"""Price caller-bound and canonical markers plus browser definition deferral."""

from __future__ import annotations

import argparse
from bisect import bisect_left, bisect_right
from collections import Counter, defaultdict
from contextlib import closing
from dataclasses import dataclass, field
from itertools import groupby
import json
from pathlib import Path
import sqlite3
import sys

from replay_round_2 import baseline, old_marker, recover_sweeps
from replay_round_3 import ExactTranscript, exact_patch_replacement, local_estimate, revised_marker

BROWSER_BUNDLE_BYTES = 11267
DISCOVERY_FRESH_TOKENS = 256
CACHE_READ_WEIGHTS = (0.10, 0.50)
READ_TOOL = "read_conversation"


def canonical_marker(conv: str, mid: str) -> str:
    return f"[tool result cleared; read_conversation @transcript:{conv}#message-{mid}]"


@dataclass(frozen=True, slots=True)
class MarkerForms:
    canonical_size: int
    live_patch_size: int = 0
    live_patch_local: int = 0
    canonical_patch_size: int = 0
    canonical_patch_local: int = 0


@dataclass(slots=True)
class Round4Transcript(ExactTranscript):
    forms: dict[int, MarkerForms] = field(default_factory=dict)
    compaction: list[tuple[int, int]] = field(default_factory=list)
    browser_first: int | None = None
    per_request_accounting: dict[int, tuple[int, int, int, int]] = field(default_factory=dict)

    def add(self, row, hints, image_counts):
        _, _, seq, kind, _, usage, _ = row
        if kind == "continuation":
            accounting = baseline.object_value(baseline.parse(usage))
            if accounting:
                total = sum(baseline.usage_key(accounting)[:3])
                if total > 0:
                    self.compaction.append((seq, total))
        ExactTranscript.add(self, row, hints, image_counts)

    def add_assistant(self, seq, raw, data, usage, created_at, hints):
        ExactTranscript.add_assistant(self, seq, raw, data, usage, created_at, hints)
        accounting = baseline.object_value(baseline.parse(usage))
        if accounting:
            key = baseline.usage_key(accounting)
            if sum(key[:3]) > 0:
                self.per_request_accounting[seq] = key
        for block in data if isinstance(data, list) else ():
            call = baseline.object_value(block)
            name = call.get("name")
            if (call.get("type") == "tool_use" and isinstance(name, str)
                    and name in baseline.NATIVE and name.startswith("browser_")
                    and self.browser_first is None):
                self.browser_first = seq

    def add_result(self, mid, seq, body, image_counts):
        ExactTranscript.add_result(self, mid, seq, body, image_counts)
        canonical = canonical_marker(self.conversation_id, mid)
        forms = MarkerForms(len(canonical.encode()))
        if self.costs[seq].patch_shape == "preview":
            content = body["content"]
            _, live = exact_patch_replacement(content, revised_marker(seq))
            _, handoff = exact_patch_replacement(content, canonical)
            forms = MarkerForms(len(canonical.encode()), len(live.encode()), local_estimate(len(live)),
                                len(handoff.encode()), local_estimate(len(handoff)))
        self.forms[seq] = forms


@dataclass(frozen=True, slots=True)
class Definition:
    full_size: int
    stub_size: int
    deferred: bool


def definition(name, description, schema, deferred):
    full = baseline.compact({"name": name, "description": description,
                             "input_schema": baseline.parse(schema), "defer_loading": bool(deferred)})
    stub = baseline.compact({"name": name, "description": description, "defer_loading": True})
    return Definition(len(full), len(stub), bool(deferred))


def read_tool_surfaces(db):
    contexts = {row[0]: {} for row in db.execute("SELECT conversation_id FROM conversation_tool_contexts")}
    for conv, name, description, schema, deferred in db.execute(
        """SELECT conversation_id,name,description,input_schema,defer_loading
           FROM conversation_tool_context_initial WHERE name='read_conversation' OR name GLOB 'browser_*'"""
    ):
        contexts[conv][name] = definition(name, description, schema, deferred)
    changes = defaultdict(list)
    for conv, seq, kind, name, description, schema, deferred in db.execute(
        """SELECT c.conversation_id,m.sequence_id,c.kind,c.name,c.description,c.input_schema,c.defer_loading
           FROM conversation_tool_context_changes c JOIN messages m ON m.message_id=c.after_message_id
           WHERE c.name='read_conversation' OR c.name GLOB 'browser_*'
           ORDER BY c.conversation_id,m.sequence_id,c.ordinal"""
    ):
        value = definition(name, description, schema, deferred) if kind == "addition" else None
        changes[conv].append((seq, name, value))
    latest = {row[0]: {} for row in db.execute("SELECT DISTINCT conversation_id FROM conversation_advertised_tools")}
    for conv, name, description, schema, deferred in db.execute(
        """SELECT d.conversation_id,d.name,d.description,d.input_schema,d.defer_loading
           FROM conversation_tool_definitions d JOIN conversation_advertised_tools a USING(conversation_id,name)
           WHERE d.name='read_conversation' OR d.name GLOB 'browser_*'"""
    ):
        latest[conv][name] = definition(name, description, schema, deferred)
    return contexts, changes, latest


def dynamic_surfaces(start, changes, requests):
    tools = start.copy()
    i = 0
    modified = True
    for req in requests:
        while i < len(changes) and changes[i][0] < req.seq:
            _, name, definition = changes[i]
            if definition is None:
                tools.pop(name, None)
            else:
                tools[name] = definition
            i += 1
            modified = True
        if modified:
            browser = [d for name, d in tools.items() if name.startswith("browser_") and not d.deferred]
            total = sum(d.full_size for d in browser)
            hosted = sum(max(0, d.full_size - d.stub_size) for d in browser)
            modified = False
        yield req, READ_TOOL in tools, total, hosted


def cache_split(stats, tag, saved, request):
    fresh, cache_write, cache_read, _ = request
    total = fresh + cache_write + cache_read
    if total <= 0:
        return
    stats[f"{tag}_fresh"] += saved * fresh / total
    stats[f"{tag}_write"] += saved * cache_write / total
    stats[f"{tag}_read"] += saved * cache_read / total


@dataclass(slots=True)
class Round4:
    corpus: Counter = field(default_factory=Counter)
    clearing: baseline.Reconstruction = field(default_factory=baseline.Reconstruction)
    live: Counter = field(default_factory=Counter)
    continuity: Counter = field(default_factory=Counter)
    proof: dict[str, Counter] = field(default_factory=lambda: defaultdict(Counter))
    browser: dict[str, Counter] = field(default_factory=lambda: defaultdict(Counter))

    def add(self, transcript, contexts, changes, latest):
        slope, _ = baseline.calibrate(transcript.requests)
        baseline.reconstruct(transcript, slope, self.clearing)
        sweeps, cleared_positions = recover_sweeps(transcript)
        cutoffs = [sweep.cutoff for sweep in sweeps]
        requests = transcript.requests
        self.corpus.update({"transcripts": 1, "ordinary_anchors": len(requests),
                            "compaction_anchors": len(transcript.compaction),
                            "compaction_anchor_input": sum(tokens for _, tokens in transcript.compaction)})
        if transcript.browser_first is not None:
            self.corpus["browser_calling_transcripts"] += 1
        patch_positions = {}
        for item in transcript.results:
            cost = transcript.costs[item.seq]
            forms = transcript.forms[item.seq]
            if item.seq in cleared_positions:
                turns = item.upper_carries - item.carries
                self.live.update({"whole_marker_turns": turns, "whole_savings":
                                  (cost.old_marker_bytes - cost.revised_marker_bytes) * slope * turns})
            if cost.patch_shape != "preview":
                continue
            sweep_index = bisect_left(cutoffs, item.seq)
            if sweep_index >= len(sweeps):
                continue
            position = sweeps[sweep_index].index
            turns = len(requests) - position
            if cost.patch_cleared_local < cost.full_local:
                self.live.update({"b_patch_marker_turns": turns, "b_savings":
                                  ((item.size - cost.patch_cleared_bytes) * slope +
                                   baseline.IMAGE_TOKENS * item.images) * turns,
                                  "patch_marker_savings":
                                  (cost.patch_cleared_bytes - forms.live_patch_size) * slope * turns})
            elif forms.live_patch_local < cost.full_local:
                self.live.update({"added_patch_marker_turns": turns, "added_patch_rows": 1,
                                  "added_patch_savings":
                                  ((item.size - forms.live_patch_size) * slope +
                                   baseline.IMAGE_TOKENS * item.images) * turns})
            if forms.live_patch_local < cost.full_local:
                patch_positions[item.seq] = position

        first_marker = min((*cleared_positions.values(), *patch_positions.values()), default=len(requests))
        self.check_context(transcript, slope, first_marker, contexts, changes, latest)
        self.add_compaction(transcript, slope, cleared_positions, patch_positions)

    def check_context(self, transcript, slope, first_marker, contexts, changes, latest):
        conv = transcript.conversation_id
        if conv in contexts:
            cohort, starting, edits = "positioned_history", contexts[conv], changes.get(conv, ())
        elif conv in latest:
            cohort, starting, edits = "latest_snapshot_proxy", latest[conv], ()
        else:
            cohort, starting, edits = "no_tool_snapshot", {}, ()
        stats = self.browser[cohort]
        proof = self.proof[cohort]
        stats["transcripts"] += 1
        browser_first = transcript.browser_first
        if browser_first is not None:
            stats["browser_calling_transcripts"] += 1
        cost_accounted = False
        any_browser = False
        for i, (request, reader, full, hosted) in enumerate(dynamic_surfaces(starting, edits, transcript.requests)):
            accounting = transcript.per_request_accounting[request.seq]
            stats["requests"] += 1
            after_marker = i >= first_marker
            if after_marker:
                proof["requests_with_markers"] += 1
                if not reader and cohort != "no_tool_snapshot":
                    proof["requests_without_advertised_reader"] += 1
                    proof[f"missing_reader_{transcript.role}"] += 1
            if full:
                any_browser = True
                stats["requests_with_browser_definitions"] += 1
            if full and (browser_first is None or request.seq <= browser_first):
                stats["requests_before_or_through_first_browser_call"] += 1
                stats["with_any_cache_read"] += accounting[2] > 0
                stats["input_on_saving_requests"] += request.total
                stats["read_on_saving_requests"] += accounting[2]
                cache_split(stats, "full_saved", full * slope, accounting)
                cache_split(stats, "stub_saved", hosted * slope, accounting)
            if (not cost_accounted and browser_first is not None and request.seq >= browser_first):
                stats["discovery_requests_priced"] += bool(full)
                if full:
                    cache_split(stats, "extra", request.total, accounting)
                    stats["extra_fresh"] += DISCOVERY_FRESH_TOKENS
                cost_accounted = True
            if browser_first is None or request.seq <= browser_first:
                stats["blanket_full_bundle_gross"] += BROWSER_BUNDLE_BYTES * slope
            if browser_first is not None and request.seq == browser_first:
                stats["blanket_one_discovery_cost"] += request.total + DISCOVERY_FRESH_TOKENS
        stats["transcripts_with_browser_definitions"] += any_browser

    def add_compaction(self, transcript, slope, old_positions, patch_positions):
        requests = transcript.requests
        cleared = old_positions | patch_positions
        if not cleared or not transcript.compaction:
            return
        risk_requests = 0
        for seq, _ in transcript.compaction:
            grew = False
            for item in transcript.results:
                index = cleared.get(item.seq)
                if index is None or requests[index].seq >= seq:
                    continue
                cost = transcript.costs[item.seq]
                forms = transcript.forms[item.seq]
                self.continuity["candidate_marker_occurrences"] += 1
                if item.seq in old_positions:
                    self.continuity["canonical_savings_ceiling"] += (
                        cost.old_marker_bytes - forms.canonical_size) * slope
                else:
                    if cost.patch_cleared_local < cost.full_local:
                        self.continuity["canonical_savings_ceiling"] += (
                            cost.patch_cleared_bytes - forms.canonical_patch_size) * slope
                    if forms.canonical_patch_local >= cost.full_local:
                        grew = True
                        self.continuity["nonstrict_patch_occurrences_if_retained_in_suffix"] += 1
                        self.continuity["growing_patch_occurrences_if_retained_in_suffix"] += (
                            forms.canonical_patch_local > cost.full_local)
            risk_requests += grew
        self.continuity["compactions_exposed_to_patch_gate_mismatch_upper"] += risk_requests

    def report(self, billing, compaction_ledger):
        ledger = sum(billing[1:])
        live = dict(self.live)
        live["a_live_savings"] = sum(live.get(k,0) for k in (
            "whole_savings","patch_marker_savings","added_patch_savings"))
        live["a_live_share"] = baseline.ratio(live["a_live_savings"], ledger)
        live["b_plus_a_live_savings"] = live["a_live_savings"] + live.get("b_savings",0)
        live["b_plus_a_live_share"] = baseline.ratio(live["b_plus_a_live_savings"], ledger)
        continuity = {**self.continuity, "classified_ledger_attempts": compaction_ledger[0],
                      "classified_ledger_input": compaction_ledger[1]}
        browser = {}
        for cohort, stats in self.browser.items():
            values = dict(stats)
            for prefix in ("full","stub"):
                values[f"{prefix}_net_tokens"] = sum(
                    stats[f"{prefix}_saved_{bucket}"] - stats[f"extra_{bucket}"]
                    for bucket in ("fresh","write","read"))
            values["weighted_scenarios"] = {
                str(m): {p: sum((stats[f"{p}_saved_{bucket}"] - stats[f"extra_{bucket}"]) * weight
                                for bucket,weight in (("fresh",1),("write",1.25),("read",m)))
                         for p in ("full","stub")}
                for m in CACHE_READ_WEIGHTS
            }
            browser[cohort] = values
        return {
            "ledger": dict(zip(("rows","fresh","cache_write","cache_read"), billing)) | {"recorded_input":ledger},
            "corpus": dict(self.corpus), "live_replay": live, "continuation": continuity,
            "locator_availability": dict(self.proof), "browser_deferral": browser,
            "cache_read_weights": CACHE_READ_WEIGHTS, "discovery_fresh_token_allowance": DISCOVERY_FRESH_TOKENS,
        }


def analyze(db: sqlite3.Connection) -> dict:
    billing = db.execute("""SELECT count(*),coalesce(sum(input_tokens),0),
        coalesce(sum(cache_creation_tokens),0),coalesce(sum(cache_read_tokens),0) FROM turn_usage""").fetchone()
    compaction = db.execute("""SELECT count(*),coalesce(sum(input_tokens+cache_creation_tokens+cache_read_tokens),0)
        FROM turn_usage u JOIN request_capacity_plans p ON u.capacity_plan_id=p.plan_id
        WHERE p.kind='compaction'""").fetchone()
    conversations = {r[0]: r[1:] for r in db.execute(
        "SELECT id,runtime_role,coalesce(clear_watermark,0) FROM conversations")}
    images = Counter(r[0] for r in db.execute("SELECT message_id FROM message_images"))
    anchors = defaultdict(lambda: defaultdict(list))
    for conv, created, reduction, fresh, write, read, output in db.execute(
        """SELECT u.conversation_id,u.created_at,p.reduced_local_tokens,
                  u.input_tokens,u.cache_creation_tokens,u.cache_read_tokens,u.output_tokens
           FROM turn_usage u JOIN request_capacity_plans p ON u.capacity_plan_id=p.plan_id
           WHERE p.kind='ordinary' AND p.reduction_action IN ('none','clearing')"""
    ):
        anchors[conv][(fresh,write,read,output)].append((baseline.timestamp(created),reduction))
    contexts, changes, latest = read_tool_surfaces(db)
    replay = Round4()
    rows = db.execute("""SELECT conversation_id,message_id,sequence_id,message_type,content,usage_data,created_at
                        FROM messages ORDER BY conversation_id,sequence_id""")
    for idx, (conv, group) in enumerate(groupby(rows,key=lambda row: row[0]),1):
        if conv not in conversations:
            continue
        transcript = Round4Transcript(*conversations[conv],conversation_id=conv)
        for row in group:
            transcript.add(row,anchors.get(conv,{}),images)
        replay.add(transcript,contexts,changes,latest)
        if idx % 2500 == 0:
            print(f"Replayed {idx} transcripts.",file=sys.stderr)
    return replay.report(billing,compaction)


def replay_logged_compactions(db: sqlite3.Connection) -> dict:
    """Approximate canonical marker exposure at the ledger's compaction times."""
    attempts = defaultdict(list)
    for conv, created, tokens in db.execute(
        """SELECT u.conversation_id,u.created_at,
                  u.input_tokens+u.cache_creation_tokens+u.cache_read_tokens
           FROM turn_usage u JOIN request_capacity_plans p ON u.capacity_plan_id=p.plan_id
           WHERE p.kind='compaction'"""
    ):
        attempts[conv].append((baseline.timestamp(created), tokens))
    conversations = {r[0]: r[1:] for r in db.execute(
        "SELECT id,runtime_role,coalesce(clear_watermark,0) FROM conversations")}
    images = Counter(r[0] for r in db.execute("SELECT message_id FROM message_images"))
    hints = defaultdict(lambda: defaultdict(list))
    for conv, created, reduction, fresh, write, read, output in db.execute(
        """SELECT u.conversation_id,u.created_at,p.reduced_local_tokens,
                  u.input_tokens,u.cache_creation_tokens,u.cache_read_tokens,u.output_tokens
           FROM turn_usage u JOIN request_capacity_plans p ON u.capacity_plan_id=p.plan_id
           WHERE p.kind='ordinary' AND p.reduction_action IN ('none','clearing')"""
    ):
        if conv in attempts:
            hints[conv][(fresh, write, read, output)].append((baseline.timestamp(created), reduction))
    replay = Round4()
    for conv, events in attempts.items():
        if conv not in conversations:
            continue
        transcript = Round4Transcript(*conversations[conv], conversation_id=conv)
        times = []
        for row in db.execute(
            """SELECT conversation_id,message_id,sequence_id,message_type,content,usage_data,created_at
               FROM messages WHERE conversation_id=? ORDER BY sequence_id""",(conv,)
        ):
            transcript.add(row, hints[conv], images)
            times.append((baseline.timestamp(row[-1]), row[2]))
        times.sort()
        timestamps = [time for time, _ in times]
        greatest_sequence = -1
        before = []
        for _, sequence in times:
            greatest_sequence = max(greatest_sequence, sequence)
            before.append(greatest_sequence)
        transcript.compaction = []
        for time, tokens in events:
            i = bisect_right(timestamps, time) - 1
            if i >= 0 and tokens > 0:
                transcript.compaction.append((before[i] + 1, tokens))
        replay.add(transcript, {}, {}, {})
    return {"dated_ledger_attempts":sum(map(len,attempts.values())),
            "matched_time_to_history_attempts": replay.corpus["compaction_anchors"],
            "matched_time_input": replay.corpus["compaction_anchor_input"], **replay.continuity}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("database",type=Path)
    parser.add_argument("--json",required=True,type=Path,help="write a new aggregate-only file")
    parser.add_argument("--logged-compactions-only", action="store_true",
                        help="price canonical form at recorded compaction times")
    args=parser.parse_args()
    if args.json.exists():
        parser.error("select a new JSON destination")
    with closing(baseline.open_read_only(args.database.expanduser())) as db:
        result=replay_logged_compactions(db) if args.logged_compactions_only else analyze(db)
    with args.json.open("x",encoding="utf-8") as output:
        output.write(json.dumps(result,indent=2,allow_nan=False)+"\n")
    return 0


if __name__=="__main__":
    raise SystemExit(main())
