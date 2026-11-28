#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.13"
# dependencies = []
# ///
"""Replay browser catalog projection and the exact deferred-tools prompt hint."""

from collections import Counter, defaultdict
from contextlib import closing
from dataclasses import dataclass, field
from functools import cache
from itertools import groupby
import argparse
import json
from pathlib import Path
import re
import sys

from replay_round_4 import Round4Transcript
from replay_round_5 import BrowserTool, baseline, route_label


MODERN_RESPONSES = frozenset({
    "gpt-6.2-astra", "gpt-6.2-sol", "gpt-6.1-terra", "gpt-6-astra",
    "gpt-6.1-sol", "gpt-6-sol", "gpt-6-luna", "gpt-5.6", "gpt-5.6-sol",
    "gpt-5.6-luna", "gpt-5.6-terra", "gpt-5.6-2026-07-01",
})
GRACE_TEXT = "You have reached your turn limit. Only submit_result or submit_error"
SCENARIOS = ("one_explicit_limit_20", "two_bundle_searches", "default_with_targeted_followups")


@cache
def definition(name, description, raw_schema, deferred):
    return BrowserTool(name, description, baseline.object_value(baseline.parse(raw_schema)), bool(deferred))


def is_browser(name):
    return name.startswith("browser_")


@cache
def hint_bytes(names):
    if not names:
        return 0
    shown = ", ".join(names[:40])
    omitted = len(names) - 40
    suffix = f" and {omitted} more" if omitted > 0 else ""
    text = (
        f"\n\nNot yet loaded, available through tool search: {shown}{suffix}."
        " Search for a tool by name or task before calling it."
    )
    return len(text.encode())


def score_tools(catalog, query, limit):
    normalized = query.lower()
    terms = set(re.findall(r"[^\W_]+", normalized))
    matches = []
    for tool in catalog:
        name = tool.name.lower()
        description = tool.description.lower()
        schema = baseline.compact(tool.parameters).decode().lower()
        score = 10_000 if name == normalized else 0
        for term in terms:
            score += 100 * (term in name) + 10 * (term in description) + (term in schema)
        if score:
            matches.append((score, tool))
    matches.sort(key=lambda item: (-item[0], item[1].name))
    return [tool for _, tool in matches[:limit]]


def exchange_bytes(tools, query, limit, call_size):
    call_id = "a" * call_size
    arguments = {"query": query}
    if limit is not None:
        arguments["limit"] = limit
    call = {"type": "tool_search_call", "execution": "client", "call_id": call_id, "arguments": arguments}
    result = {"type": "tool_search_output", "execution": "client", "call_id": call_id,
              "status": "completed", "tools": [tool.wire(True) for tool in tools]}
    return sum(len(baseline.compact(item)) + 1 for item in (call, result))


@dataclass(slots=True)
class DiscoveryState:
    loaded: set = field(default_factory=set)
    bytes: int = 0
    rounds: int = 0

    def search(self, catalog, query, limit, call_size):
        tools = score_tools(catalog, query, 8 if limit is None else limit)
        self.bytes += exchange_bytes(tools, query, limit, call_size)
        self.loaded.update(tool.name for tool in tools)
        self.rounds += 1


@dataclass(slots=True)
class ResponseTranscript(Round4Transcript):
    times: dict = field(default_factory=dict)
    browser_calls: dict = field(default_factory=dict)
    grace_sequence: int | None = None

    def add(self, row, hints, images):
        _, _, seq, kind, raw, _, _ = row
        if self.role == "sub_agent" and kind == "user" and GRACE_TEXT in raw:
            self.grace_sequence = seq
        Round4Transcript.add(self, row, hints, images)

    def add_assistant(self, seq, raw, data, usage, created_at, hints):
        Round4Transcript.add_assistant(self, seq, raw, data, usage, created_at, hints)
        if seq in self.per_request_accounting:
            self.times[seq] = baseline.timestamp(created_at)
        if "browser_" in raw and isinstance(data, list):
            names = tuple(block["name"] for block in data if isinstance(block, dict)
                          and block.get("type") == "tool_use"
                          and isinstance(block.get("name"), str) and is_browser(block["name"]))
            if names:
                self.browser_calls[seq] = names


def read_policy(db):
    context = dict(db.execute("""
        SELECT conversation_id,route_key LIKE '%:codex-account=%' FROM conversation_tool_contexts
    """))
    initial = {conversation: {} for conversation in context}
    latest = {r[0]: {} for r in db.execute("SELECT DISTINCT conversation_id FROM conversation_callable_tools")}
    search_catalogs = {row[0] for row in db.execute("""
        SELECT conversation_id FROM conversation_callable_tools WHERE name='tool_search'
        UNION SELECT conversation_id FROM conversation_tool_context_initial WHERE name='tool_search'
    """)}
    for destination, sql in (
        (initial, """SELECT conversation_id,name,description,input_schema,defer_loading
             FROM conversation_tool_context_initial WHERE name GLOB 'browser_*' OR defer_loading=1"""),
        (latest, """SELECT conversation_id,name,description,input_schema,defer_loading
             FROM conversation_tool_definitions JOIN conversation_callable_tools USING(conversation_id,name)
             WHERE name GLOB 'browser_*' OR defer_loading=1"""),
    ):
        for conv, name, description, schema, deferred in db.execute(sql):
            destination[conv][name] = definition(name, description, schema, deferred)
    changes = defaultdict(list)
    for conv, seq, kind, name, description, schema, deferred in db.execute("""
        SELECT c.conversation_id,m.sequence_id,c.kind,c.name,c.description,c.input_schema,c.defer_loading
        FROM conversation_tool_context_changes c JOIN messages m ON m.message_id=c.after_message_id
        WHERE c.name GLOB 'browser_*' OR c.defer_loading=1 OR c.kind='removal'
        ORDER BY c.conversation_id,m.sequence_id,c.ordinal
    """):
        tool = definition(name, description, schema, deferred) if kind == "addition" else None
        changes[conv].append((seq, name, tool))
    return context, initial, latest, changes, search_catalogs


def read_usage(db):
    hints = defaultdict(lambda: defaultdict(list))
    evidence = defaultdict(lambda: defaultdict(list))
    routes = defaultdict(Counter)
    for conv, stamp, model, route, kind, action, reduction, *tokens in db.execute("""
        SELECT u.conversation_id,u.created_at,u.model,p.route_class,p.kind,p.reduction_action,
               p.reduced_local_tokens,u.input_tokens,u.cache_creation_tokens,u.cache_read_tokens,u.output_tokens
        FROM turn_usage u LEFT JOIN request_capacity_plans p ON p.plan_id=u.capacity_plan_id
        WHERE p.kind='ordinary' OR p.kind IS NULL
    """):
        time = baseline.timestamp(stamp)
        key = tuple(tokens)
        evidence[conv][key].append((time, model in MODERN_RESPONSES, route))
        routes[route or "unlinked"]["ordinary_or_legacy_usage_attempts"] += 1
        if kind and action in {"none", "clearing"}:
            hints[conv][key].append((time, reduction))
    return hints, evidence, routes


def match_route(usage, when, codex_context):
    near = sorted((abs(stamp - when), modern, route or "") for stamp, modern, route in usage
                  if abs(stamp - when) <= 120)
    if not near or (len(near) > 1 and near[1][0] - near[0][0] < 1):
        return None
    _, modern, route = near[0]
    if not modern:
        return None
    if route == "codex_auth":
        return "matched_codex"
    if not route and codex_context:
        return "codex_context_proxy"
    return None


def select_catalog(current):
    browser = [tool for tool in current.values() if is_browser(tool.name)]
    catalog = tuple(tool for tool in current.values() if tool.deferred or is_browser(tool.name))
    names = tuple(sorted(tool.name for tool in catalog))
    eager = sum(len(baseline.compact(tool.wire(False))) + 1 for tool in browser if not tool.deferred)
    return browser, catalog, names, hint_bytes(names), eager


def price_search(state, params, stats, req, eager, hint, slope, cache_share, catalog, call_size):
    query, limit = params
    input_tokens = max(0, req.total + (hint - eager + state.bytes) * slope)
    stats["extra_search_requests"] += 1
    stats["extra_search_input"] += input_tokens
    stats["extra_search_cache"] += input_tokens * cache_share
    state.search(catalog, query, limit, call_size)


def price_transcript(t, usage, scope, current, changes, codex_context, known_search, call_size, totals, diagnostics):
    route = route_label(t.role, scope)
    slope, _ = baseline.calibrate(t.requests)
    current = current.copy()
    changes = iter(changes)
    next_change = next(changes, None)
    states = {key: DiscoveryState() for key in SCENARIOS}
    browser, catalog, names, hint, eager = select_catalog(current)
    eligible_browser_transcript = False
    for req in t.requests:
        changed = False
        while next_change and next_change[0] < req.seq:
            _, name, tool = next_change
            if tool is None:
                current.pop(name, None)
            else:
                current[name] = tool
            next_change = next(changes, None)
            changed = True
        if changed:
            browser, catalog, names, hint, eager = select_catalog(current)
        accounting = t.per_request_accounting[req.seq]
        match = match_route(usage.get(accounting, ()), t.times[req.seq], codex_context)
        if match is None:
            diagnostics[route]["unclassified_or_eager_anchors"] += 1
            continue
        if t.grace_sequence is not None and t.grace_sequence < req.seq:
            diagnostics[route]["terminal_anchors"] += 1
            continue
        if not browser and not known_search:
            diagnostics[route]["without_callable_search_evidence"] += 1
            continue
        diagnostics[route][match] += 1
        if not hint:
            continue
        diagnostics[route]["hint_anchors"] += 1
        diagnostics[route]["hint_without_browser_anchors"] += not browser
        diagnostics[route]["omitted_hint_names"] += max(0, len(catalog) - 40)
        diagnostics[route]["hints_missing_browser_names"] += any(is_browser(name) for name in names[40:])
        cache_share = baseline.ratio(accounting[2], req.total)
        eligible_names = {tool.name for tool in browser if not tool.deferred}
        current_calls = tuple(name for name in t.browser_calls.get(req.seq, ()) if name in eligible_names)
        if current_calls and not eligible_browser_transcript:
            eligible_browser_transcript = True
            first_results = {tool.name for tool in score_tools(catalog, "browser", 8)}
            diagnostics[route]["browser_transcripts"] += 1
            diagnostics[route]["first_request_missing_from_default_browser_results"] += (
                not set(current_calls).issubset(first_results)
            )
        for scenario, state in states.items():
            costs = Counter()
            if current_calls and state.rounds == 0:
                limit = 20 if scenario == SCENARIOS[0] else None
                price_search(state, ("browser", limit), costs, req, eager, hint, slope, cache_share, catalog, call_size)
                if scenario == SCENARIOS[1]:
                    price_search(state, ("browser", 20), costs, req, eager, hint, slope, cache_share, catalog, call_size)
            if scenario == SCENARIOS[2]:
                for name in current_calls:
                    if name not in state.loaded:
                        price_search(state, (name, None), costs, req, eager, hint, slope, cache_share, catalog, call_size)
            costs["missing_needed_tools"] += sum(name not in state.loaded for name in current_calls)
            costs.update({
                "requests": 1, "schema_removed": eager * slope, "hint_input": hint * slope,
                "search_replayed": state.bytes * slope,
                "schema_removed_cache": eager * slope * cache_share,
                "hint_cache": hint * slope * cache_share,
                "search_replayed_cache": state.bytes * slope * cache_share,
            })
            for cohort in ("route_or_context", match):
                totals[scenario, cohort, route].update(costs)
    if eligible_browser_transcript:
        rounds = states[SCENARIOS[2]].rounds
        diagnostics[route]["targeted_one_search"] += rounds == 1
        diagnostics[route]["targeted_two_searches"] += rounds == 2
        diagnostics[route]["targeted_three_or_more"] += rounds > 2


def followup_baseline():
    with Path(__file__).with_name("round-6.json").open() as source:
        previous = json.load(source)["parent_by_route"]
    return {route: {period: {
        **counts,
        "browser_calls_per_1000": 1000 * counts["browser_calls"] / counts["requests"] if counts["requests"] else None,
        "ask_user_question_calls_per_1000": 1000 * counts["ask_user_question_calls"] / counts["requests"] if counts["requests"] else None,
    } for period, counts in windows.items()} for route, windows in previous.items()}


def analyze(db):
    total_input = db.execute("SELECT coalesce(sum(input_tokens+cache_creation_tokens+cache_read_tokens),0) FROM turn_usage").fetchone()[0]
    conversations = dict((cid, (role, watermark, kind)) for cid, role, watermark, kind in db.execute(
        "SELECT id,runtime_role,coalesce(clear_watermark,0),cm_kind FROM conversations"
    ))
    context, initial, latest, edits, search_catalogs = read_policy(db)
    hints, evidence, routes = read_usage(db)
    images = Counter(row[0] for row in db.execute("SELECT message_id FROM message_images"))
    call_sizes = sorted(row[0] for row in db.execute("SELECT length(tool_call_id) FROM tool_discovery_results"))
    call_size = call_sizes[len(call_sizes) // 2] if call_sizes else 36
    totals = defaultdict(Counter)
    diagnostics = defaultdict(Counter)
    sql = """SELECT conversation_id,message_id,sequence_id,message_type,content,usage_data,created_at
             FROM messages ORDER BY conversation_id,sequence_id"""
    for index, (conv, rows) in enumerate(groupby(db.execute(sql), key=lambda row: row[0]), 1):
        if conv not in conversations:
            continue
        role, watermark, scope = conversations[conv]
        if conv in initial:
            start = initial[conv]
            changes = edits.get(conv, ())
        elif conv in latest:
            start = latest[conv]
            changes = ()
        else:
            continue
        t = ResponseTranscript(role, watermark, conversation_id=conv)
        for row in rows:
            t.add(row, hints.get(conv, {}), images)
        price_transcript(t, evidence.get(conv, {}), scope, start, changes, context.get(conv, False),
                         conv in search_catalogs, call_size, totals, diagnostics)
        if index % 2500 == 0:
            print(f"Replayed {index} transcripts.", file=sys.stderr, flush=True)
    scenarios = defaultdict(lambda: defaultdict(dict))
    for (scenario, cohort, surface), v in sorted(totals.items()):
        net = v["schema_removed"] - v["hint_input"] - v["search_replayed"] - v["extra_search_input"]
        cache_net = v["schema_removed_cache"] - v["hint_cache"] - v["search_replayed_cache"] - v["extra_search_cache"]
        scenarios[scenario][cohort][surface] = dict(v) | {"net_input": net, "net_cache_read": cache_net}
    return {"recorded_input": total_input, "provider_coverage": dict(routes), "call_id_bytes": call_size,
            "diagnostics": dict(diagnostics), "scenarios": dict(scenarios), "baseline": followup_baseline()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("database", type=Path)
    parser.add_argument("--json", required=True, type=Path)
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
