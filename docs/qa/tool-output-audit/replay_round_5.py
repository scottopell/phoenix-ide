#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.13"
# dependencies = []
# ///
"""Measure production tool discovery and replay the client browser bundle."""

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

from replay_round_2 import baseline, recover_sweeps
from replay_round_3 import exact_patch_replacement, local_estimate
from replay_round_4 import Round4Transcript


@dataclass(frozen=True, slots=True)
class BrowserTool:
    name: str
    description: str
    parameters: dict
    deferred: bool

    def wire(self, deferred: bool) -> dict:
        function = {"type": "function", "name": self.name, "description": self.description,
                    "parameters": self.parameters}
        if deferred:
            function["defer_loading"] = True
        return function


def browser_tool(name, description, schema, deferred):
    return BrowserTool(name, description, baseline.object_value(baseline.parse(schema)), bool(deferred))


def wire_bundle(tools: tuple[BrowserTool, ...], call_id_size: int) -> tuple[int, int]:
    eager_bytes = sum(len(baseline.compact(t.wire(False))) + 1 for t in tools)
    call_id = "a" * call_id_size
    search_call = {"type": "tool_search_call", "execution": "client", "call_id": call_id,
                   "arguments": {"query": "browser", "limit": 20}}
    search_output = {"type": "tool_search_output", "execution": "client", "call_id": call_id,
                     "status": "completed", "tools": [t.wire(True) for t in tools]}
    versioned_exchange = sum(len(baseline.compact(item)) + 1 for item in (search_call, search_output))
    return eager_bytes, versioned_exchange


def route_label(role: str, mode: str | None) -> str:
    if role == "coordinator":
        return "coordinator"
    if role != "sub_agent":
        return "ordinary_parent"
    if mode in {"attached_work_child", "work", "branch", "detached_approved_task"}:
        return "subagent_work"
    if mode == "explore":
        return "subagent_explore"
    return "subagent_legacy_mode"


def read_browser_surfaces(db):
    initial = {r[0]: {} for r in db.execute("SELECT conversation_id FROM conversation_tool_contexts")}
    latest = {r[0]: {} for r in db.execute("SELECT DISTINCT conversation_id FROM conversation_advertised_tools")}
    for target, sql in (
        (initial, """SELECT conversation_id,name,description,input_schema,defer_loading
            FROM conversation_tool_context_initial WHERE name GLOB 'browser_*'"""),
        (latest, """SELECT d.conversation_id,d.name,d.description,d.input_schema,d.defer_loading
            FROM conversation_tool_definitions d JOIN conversation_advertised_tools a USING(conversation_id,name)
            WHERE d.name GLOB 'browser_*'"""),
    ):
        for conv, name, desc, schema, deferred in db.execute(sql):
            target[conv][name] = browser_tool(name,desc,schema,deferred)
    changes = defaultdict(list)
    for conv, seq, kind, name, desc, schema, deferred in db.execute(
        """SELECT c.conversation_id,m.sequence_id,c.kind,c.name,c.description,c.input_schema,c.defer_loading
           FROM conversation_tool_context_changes c JOIN messages m ON m.message_id=c.after_message_id
           WHERE c.name GLOB 'browser_*' ORDER BY c.conversation_id,m.sequence_id,c.ordinal"""
    ):
        changes[conv].append((seq,name,browser_tool(name,desc,schema,deferred) if kind == "addition" else None))
    return initial, changes, latest


@dataclass(slots=True)
class Observation(Round4Transcript):
    observed_search_calls: set[tuple[str, str]] = field(default_factory=set)
    relevant_deferred: frozenset[str] = frozenset()
    deferred_calls: dict[str, tuple[str, int]] = field(default_factory=dict)
    deferred_successes: dict[str, list[int]] = field(default_factory=lambda: defaultdict(list))
    new_live_patch: dict[int, tuple[int, int]] = field(default_factory=dict)
    current_mid: str = ""

    def add(self, row, hints, image_counts):
        self.current_mid = row[1]
        Round4Transcript.add(self,row,hints,image_counts)

    def add_assistant(self, seq, raw, data, usage, created_at, hints):
        Round4Transcript.add_assistant(self,seq,raw,data,usage,created_at,hints)
        for block in data if isinstance(data,list) else ():
            call = baseline.object_value(block)
            if call.get("type") != "tool_use":
                continue
            name, identity = call.get("name"), call.get("id")
            if name in {"tool_search","tool_search_tool"} and isinstance(identity,str):
                self.observed_search_calls.add((self.current_mid,identity))
            if name in self.relevant_deferred and isinstance(identity,str):
                self.deferred_calls[identity] = (name,seq)

    def add_result(self, mid, seq, body, image_counts):
        Round4Transcript.add_result(self,mid,seq,body,image_counts)
        direct = self.deferred_calls.get(body.get("tool_use_id"))
        if direct and not bool(body.get("is_error")):
            name, origin = direct
            self.deferred_successes[name].append(origin)
        if self.costs[seq].patch_shape == "preview":
            _, text = exact_patch_replacement(body["content"], f"[tool result cleared; source @self#seq-{seq}]")
            self.new_live_patch[seq] = len(text.encode()), local_estimate(len(text))


@dataclass(frozen=True, slots=True)
class Discovery:
    owner: str
    call: str
    result_sequence: int
    names: tuple[str, ...]


def load_discovery(db):
    by_conv = defaultdict(list)
    for conv, owner, call, seq, encoded in db.execute(
        """SELECT d.conversation_id,d.owner_message_id,d.tool_call_id,m.sequence_id,
           (SELECT json_group_array(t.name) FROM tool_discovery_result_tools t
            WHERE t.result_message_id=d.result_message_id AND t.defer_loading=1)
           FROM tool_discovery_results d JOIN messages m ON m.message_id=d.result_message_id"""
    ):
        by_conv[conv].append(Discovery(owner,call,seq,tuple(json.loads(encoded))))
    return by_conv


@dataclass(slots=True)
class Audit:
    corpus: Counter = field(default_factory=Counter)
    discovery: dict[str, Counter] = field(default_factory=lambda: defaultdict(Counter))
    savings: dict[tuple[str,str], Counter] = field(default_factory=lambda: defaultdict(Counter))
    clearing: baseline.Reconstruction = field(default_factory=baseline.Reconstruction)
    markers: Counter = field(default_factory=Counter)

    def add(self, transcript, mode, tool_state, discoveries, capacity, call_size):
        route = route_label(transcript.role,mode)
        requests = transcript.requests
        slope, _ = baseline.calibrate(requests)
        self.corpus.update({"transcripts":1,"ordinary_anchors":len(requests)})
        self.track_discovery(transcript,discoveries,route)
        self.track_markers(transcript,slope)
        first = transcript.browser_first
        seqs = [r.seq for r in requests]
        first_index = bisect_left(seqs,first) if first is not None else len(seqs)
        if first is not None:
            self.discovery[route]["browser_calling_transcripts"] += 1
            if first_index < 3 and first_index < len(seqs):
                self.discovery[route]["first_browser_within_first_three_requests"] += 1
            if first_index == len(seqs):
                self.discovery[route]["browser_without_request_anchor"] += 1
        self.price_browser(transcript,slope,route,tool_state,capacity,first_index,call_size)

    def track_discovery(self, transcript, snapshots, route):
        counts = self.discovery[route]
        counts["transcripts"] += 1
        observed = transcript.observed_search_calls | {(x.owner,x.call) for x in snapshots}
        counts["search_calls"] += len(observed)
        counts["search_transcripts"] += bool(observed)
        counts["persisted_search_results"] += len(snapshots)
        counts["returned_deferred_definitions"] += sum(bool(x.names) for x in snapshots)
        for values in transcript.deferred_successes.values():
            values.sort()
        for result in snapshots:
            succeeded = any(
                len(calls) > bisect_right(calls,result.result_sequence)
                for name in result.names if (calls := transcript.deferred_successes.get(name))
            )
            counts["followed_by_successful_deferred_use"] += succeeded

    def track_markers(self,transcript,slope):
        baseline.reconstruct(transcript,slope,self.clearing)
        sweeps, positions = recover_sweeps(transcript)
        cutoffs = [sweep.cutoff for sweep in sweeps]
        nreq = len(transcript.requests)
        for item in transcript.results:
            cost = transcript.costs[item.seq]
            new_marker_size = len(f"[tool result cleared; source @self#seq-{item.seq}]".encode())
            if item.seq in positions:
                delta = cost.old_marker_bytes - new_marker_size
                self.markers["a_ordinary_whole_savings"] += delta * slope * (item.upper_carries - item.carries)
            patch = transcript.new_live_patch.get(item.seq)
            if patch is None:
                continue
            sweep = bisect_left(cutoffs,item.seq)
            if sweep == len(sweeps):
                continue
            turns = nreq - sweeps[sweep].index
            if cost.patch_cleared_local < cost.full_local:
                self.markers["b_savings"] += ((item.size-cost.patch_cleared_bytes)*slope +
                                              item.images*baseline.IMAGE_TOKENS)*turns
                self.markers["a_existing_patch_savings"] += (cost.patch_cleared_bytes-patch[0])*slope*turns
            elif patch[1] < cost.full_local:
                self.markers["a_new_patch_savings"] += ((item.size-patch[0])*slope +
                                                       item.images*baseline.IMAGE_TOKENS)*turns

    def price_browser(self, transcript, slope, route, tool_state, capacity, first_index, call_size):
        label, initial, changes = tool_state
        current = initial.copy()
        edit = 0
        loaded_bytes = 0
        priced_discovery = False
        eager = 0
        active_bundle = ()
        known = label != "no_tool_snapshot"
        any_eager = False
        for idx, request in enumerate(transcript.requests):
            if idx == 0 or (edit < len(changes) and changes[edit][0] < request.seq):
                while edit < len(changes) and changes[edit][0] < request.seq:
                    _,name,spec = changes[edit]
                    if spec is None:
                        current.pop(name,None)
                    else:
                        current[name] = spec
                    edit += 1
                active_bundle = tuple(spec for spec in current.values() if not spec.deferred)
                eager = sum(len(baseline.compact(t.wire(False)))+1 for t in active_bundle)
            accounting = transcript.per_request_accounting[request.seq]
            candidates = capacity.get(accounting, ())
            matched = len(candidates) == 1 and candidates[0]
            keys = [(route,"all_recorded")]
            if known:
                keys.append((route,label))
            if matched and known:
                keys.append((route,"matched_codex_capacity"))
            any_eager |= bool(eager)
            for key in keys:
                self.savings[key].update({"anchors":1,"with_eager_definitions":eager>0})
            if idx == first_index and eager:
                _, loaded_bytes = wire_bundle(active_bundle,call_size)
                priced_discovery = True
                discovery_input = max(0, request.total-eager*slope)
                for key in keys:
                    stats = self.savings[key]
                    stats.update({"additional_searches":1,
                                  "additional_search_input":discovery_input,
                                  "additional_search_read":baseline.ratio(discovery_input,request.total)*accounting[2]})
            for key in keys:
                if not known or (idx >= first_index and not priced_discovery):
                    continue
                delta = (eager - loaded_bytes) * slope
                stats = self.savings[key]
                stats.update({"definition_and_replay_delta":delta,
                              "definition_and_replay_delta_cache_read":
                                  baseline.ratio(accounting[2],request.total)*delta})
        self.discovery[route]["transcripts_with_eager_browser_evidence"] += any_eager

    def report(self, ledger, call_size):
        by_route = defaultdict(dict)
        for (route, evidence), counts in self.savings.items():
            net = counts["definition_and_replay_delta"] - counts["additional_search_input"]
            by_route[route][evidence] = {**counts,"net_input":net,
                "net_cache_read":counts["definition_and_replay_delta_cache_read"]-counts["additional_search_read"]}
        a = sum(value for key,value in self.markers.items() if key.startswith("a_"))
        return {"recorded_input":ledger,"corpus":dict(self.corpus),"wire_model":{
                    "browser_query_limit":20,"call_id_size_bytes":call_size},
                "observed_discovery":dict(self.discovery),"browser_replay":dict(by_route),
                "marker_replay":dict(self.markers)|{"a_live_savings":a,"b_plus_a_live_savings":a+self.markers["b_savings"]}}


def analyze(db):
    ledger = db.execute("SELECT coalesce(sum(input_tokens+cache_creation_tokens+cache_read_tokens),0) FROM turn_usage").fetchone()[0]
    conversations = {r[0]:r[1:] for r in db.execute(
        "SELECT id,runtime_role,coalesce(clear_watermark,0),cm_kind FROM conversations")}
    images = Counter(r[0] for r in db.execute("SELECT message_id FROM message_images"))
    clearing = defaultdict(lambda: defaultdict(list))
    routes = defaultdict(lambda: defaultdict(list))
    for conv, created, reduction, route, fresh, write, read, output in db.execute(
        """SELECT u.conversation_id,u.created_at,p.reduced_local_tokens,p.route_class,
                  u.input_tokens,u.cache_creation_tokens,u.cache_read_tokens,u.output_tokens
           FROM turn_usage u JOIN request_capacity_plans p ON u.capacity_plan_id=p.plan_id
           WHERE p.kind='ordinary' AND p.reduction_action IN ('none','clearing')"""
    ):
        usage = fresh,write,read,output
        clearing[conv][usage].append((baseline.timestamp(created),reduction))
        routes[conv][usage].append(route == "codex_auth")
    history, edits, latest = read_browser_surfaces(db)
    discovery = load_discovery(db)
    call_sizes = sorted(len(row.call.encode()) for series in discovery.values() for row in series)
    call_size = call_sizes[len(call_sizes)//2] if call_sizes else 36
    audit = Audit()
    rows = db.execute("""SELECT conversation_id,message_id,sequence_id,message_type,content,usage_data,created_at
                        FROM messages ORDER BY conversation_id,sequence_id""")
    for index,(conv,group) in enumerate(groupby(rows,key=lambda row:row[0]),1):
        if conv not in conversations:
            continue
        role,watermark,mode = conversations[conv]
        snapshots = discovery.get(conv,())
        wanted = frozenset(name for snap in snapshots for name in snap.names)
        transcript = Observation(role,watermark,conversation_id=conv,relevant_deferred=wanted)
        for row in group:
            transcript.add(row,clearing.get(conv,{}),images)
        if conv in history:
            tools = "positioned_history",history[conv],edits.get(conv,())
        elif conv in latest:
            tools = "latest_snapshot_proxy",latest[conv],()
        else:
            tools = "no_tool_snapshot",{},()
        audit.add(transcript,mode,tools,snapshots,routes.get(conv,{}),call_size)
        if index%2500==0:
            print(f"Replayed {index} transcripts.",file=sys.stderr)
    return audit.report(ledger,call_size)


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("database",type=Path)
    p.add_argument("--json",required=True,type=Path,help="write a new aggregate-only file")
    args=p.parse_args()
    if args.json.exists():
        p.error("select a new JSON destination")
    with closing(baseline.open_read_only(args.database.expanduser())) as db:
        result=analyze(db)
    with args.json.open("x",encoding="utf-8") as out:
        out.write(json.dumps(result,indent=2,allow_nan=False)+"\n")
    return 0


if __name__=="__main__":
    raise SystemExit(main())
