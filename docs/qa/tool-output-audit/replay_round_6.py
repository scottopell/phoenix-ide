#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.13"
# dependencies = []
# ///
"""Measure AskUserQuestion and browser calls around the discovery rollout."""

from __future__ import annotations

import argparse
from collections import Counter, defaultdict
from contextlib import closing
from dataclasses import dataclass, field
import json
from pathlib import Path
import re

from replay_round_2 import baseline


START = "2026-11-09"
CUTOVER = "2026-11-23"
MATCH_SECONDS = 120
QUESTION = "ask_user_question"
SEARCH = {"tool_search", "tool_search_tool", "tool_search_tool_regex"}
ROUTES = {
    "codex_auth": "codex",
    "open_ai_responses_direct": "openai_responses",
    "open_ai_responses_compatible": "openai_responses",
    "anthropic_direct": "anthropic",
    "anthropic_compatible": "anthropic",
}
MODEL_FAMILIES = {
    "gpt-6.2-astra": "gpt_6_2_astra",
    "gpt-6.2-sol": "gpt_6_2_sol",
    "gpt-6.1-terra": "gpt_6_1_terra",
    "gpt-6.1-sol": "gpt_6_1_sol",
    "gpt-6-astra": "gpt_6_astra",
    "gpt-6-sol": "gpt_6_sol",
    "gpt-6-luna": "gpt_6_luna",
}
QUESTION_NAME = re.compile(r"(?<![\w])ask[\s_-]+user[\s_-]+question(?![\w])", re.I)
TOKENS = re.compile(r"[\w]+")


def epoch(created_at):
    return "before" if created_at[:10] < CUTOVER else "after"


def model_family(model):
    if model in MODEL_FAMILIES:
        return MODEL_FAMILIES[model]
    if model.startswith("gpt-5"):
        return "gpt_5"
    if model.startswith("gpt-6"):
        return "other_gpt_6"
    if model.startswith("claude-"):
        return "claude"
    return "other"


def query_category(value, tool_names):
    payload = baseline.object_value(baseline.parse(value) if isinstance(value, str) else value)
    query = payload.get("query")
    paths = payload.get("paths")
    texts = [query] if isinstance(query, str) and query.strip() else []
    if isinstance(paths, list):
        texts.extend(path for path in paths if isinstance(path, str) and path.strip())
    if not texts:
        return "missing_query", False
    named_question = any(QUESTION_NAME.search(part) for part in texts)
    if named_question:
        return "named_ask_user_question", True
    if any(set(TOKENS.findall(part.lower())) & tool_names for part in texts):
        return "named_tool", False
    if any(re.search(r"\b(?:ask|clarif\w*|question\w*)\b", part, re.I) for part in texts):
        return "question_or_clarification_terms", False
    return "task_or_keyword", False


@dataclass(frozen=True, slots=True)
class Attempt:
    stamp: str
    timestamp: float
    route: str
    model: str
    role: str
    transcript: str


@dataclass(slots=True)
class Counts:
    requests: int = 0
    questions: int = 0
    browsers: int = 0
    transcripts: set[str] = field(default_factory=set)
    question_transcripts: set[str] = field(default_factory=set)
    browser_transcripts: set[str] = field(default_factory=set)

    def add(self, transcript, questions, browsers):
        self.requests += 1
        self.questions += questions
        self.browsers += browsers
        self.transcripts.add(transcript)
        if questions:
            self.question_transcripts.add(transcript)
        if browsers:
            self.browser_transcripts.add(transcript)

    def report(self):
        return {
            "requests": self.requests,
            "transcripts": len(self.transcripts),
            "ask_user_question_calls": self.questions,
            "ask_user_question_transcripts": len(self.question_transcripts),
            "browser_calls": self.browsers,
            "browser_transcripts": len(self.browser_transcripts),
        }


def load_attempts(db):
    candidates = defaultdict(list)
    attempts = {}
    sql = """
        SELECT u.id,u.conversation_id,p.created_at,u.created_at,p.route_class,
               u.model,c.runtime_role,u.input_tokens,u.cache_creation_tokens,
               u.cache_read_tokens,u.output_tokens
        FROM turn_usage u JOIN conversations c ON c.id=u.conversation_id
        JOIN request_capacity_plans p ON p.plan_id=u.capacity_plan_id
        WHERE u.created_at >= ? AND p.created_at >= ? AND p.kind='ordinary'
    """
    for row in db.execute(sql, (START, START)):
        identity, transcript, request_stamp, usage_stamp, route, model, role, *usage = row
        attempts[identity] = Attempt(request_stamp, baseline.timestamp(usage_stamp),
                                     ROUTES.get(route, "other"), model_family(model),
                                     role, transcript)
        candidates[transcript, tuple(usage)].append(identity)
    return attempts, candidates


def nearest_attempt(attempts, choices, timestamp):
    nearby = sorted((abs(attempts[key].timestamp - timestamp), key) for key in choices
                    if abs(attempts[key].timestamp - timestamp) <= MATCH_SECONDS)
    if not nearby or (len(nearby) > 1 and nearby[1][0] - nearby[0][0] < 1):
        return None
    return nearby[0][1]


def tools_in_message(raw):
    blocks = baseline.parse(raw)
    return [b for b in blocks if isinstance(b, dict)
            and b.get("type") in {"tool_use", "server_tool_use"}] if isinstance(blocks, list) else []


def load_latest_policy(db):
    return {row[0]: tuple(bool(flag) for flag in row[1:]) for row in db.execute("""
        SELECT c.id,
               EXISTS(SELECT 1 FROM conversation_callable_tools x JOIN conversation_tool_definitions d
                      USING(conversation_id,name) WHERE x.conversation_id=c.id
                      AND x.name='ask_user_question' AND d.defer_loading=1),
               EXISTS(SELECT 1 FROM conversation_advertised_tools x WHERE x.conversation_id=c.id
                      AND x.name='ask_user_question'),
               EXISTS(SELECT 1 FROM conversation_advertised_tools x WHERE x.conversation_id=c.id
                      AND x.name='tool_search')
        FROM conversations c WHERE c.runtime_role='user' AND c.updated_at >= ?
    """, (START,))}


def load_discovery_results(db):
    return {(owner, call): (result_seq, bool(has_definitions), bool(has_question))
            for owner, call, result_seq, has_definitions, has_question in db.execute("""
        SELECT d.owner_message_id,d.tool_call_id,result.sequence_id,
               EXISTS(SELECT 1 FROM tool_discovery_result_tools t
                      WHERE t.result_message_id=d.result_message_id AND t.defer_loading=1),
               EXISTS(SELECT 1 FROM tool_discovery_result_tools t
                      WHERE t.result_message_id=d.result_message_id AND t.name='ask_user_question')
        FROM tool_discovery_results d JOIN messages m ON m.message_id=d.owner_message_id
        JOIN messages result ON result.message_id=d.result_message_id
        WHERE m.created_at >= ?
    """, (START,))}


def scan_messages(db, attempts, candidates):
    roles = ("user", "coordinator", "sub_agent")
    tool_names = {name.lower() for (name,) in db.execute(
        "SELECT DISTINCT name FROM conversation_tool_definitions"
    ) if TOKENS.fullmatch(name)}
    results = load_discovery_results(db)
    matched = {}
    coverage = defaultdict(Counter)
    searches = defaultdict(lambda: defaultdict(Counter))
    sql = """
        SELECT m.message_id,m.sequence_id,m.created_at,m.usage_data,m.content
        FROM messages m
        WHERE m.conversation_id=? AND m.created_at >= ? AND m.message_type='agent'
          AND (m.usage_data IS NOT NULL OR instr(m.content,'tool_search') > 0
               OR instr(m.content,'ask_user_question') > 0 OR instr(m.content,'browser_') > 0)
    """
    for transcript, role in db.execute(
        "SELECT id,runtime_role FROM conversations WHERE updated_at >= ?", (START,)
    ):
        if role not in roles:
            continue
        question_sequences = []
        question_searches = []
        for message, sequence, stamp, raw_usage, raw_content in db.execute(sql, (transcript, START)):
            counter = coverage[epoch(stamp), role]
            usage = baseline.parse(raw_usage)
            identity = None
            if isinstance(usage, dict):
                counter["response_anchors_with_usage"] += 1
                key = transcript, baseline.usage_key(usage)
                identity = nearest_attempt(attempts, candidates.get(key, ()), baseline.timestamp(stamp))
            blocks = tools_in_message(raw_content) if raw_content and (
                QUESTION in raw_content or "browser_" in raw_content or "tool_search" in raw_content
            ) else []
            questions = sum(b.get("name") == QUESTION and b["type"] == "tool_use" for b in blocks)
            browsers = sum(str(b.get("name", "")).startswith("browser_") and
                           b["type"] == "tool_use" for b in blocks)
            if identity is None or identity in matched:
                counter["unmatched_ask_user_question_calls"] += questions
                counter["unmatched_browser_calls"] += browsers
            else:
                counter["matched_response_anchors"] += 1
                matched[identity] = questions, browsers
                if questions:
                    question_sequences.append(sequence)
            attempt = attempts.get(identity)
            route = attempt.route if attempt else "unattributed"
            period = epoch(attempt.stamp) if attempt else epoch(stamp)
            for block in blocks:
                if block.get("name") not in SEARCH:
                    continue
                category, named_question = query_category(block.get("input"), tool_names)
                s = searches[route][period, role]
                s["calls"] += 1
                s[category] += 1
                s["queries_naming_ask_user_question"] += named_question
                s["provider_server_calls"] += block["type"] == "server_tool_use"
                result = results.get((message, block.get("id")))
                if result is not None:
                    result_seq, has_definitions, has_question = result
                    s["durable_results"] += 1
                    s["results_with_deferred_tools"] += has_definitions
                    s["results_with_ask_user_question"] += has_question
                    if has_question:
                        question_searches.append((result_seq, s, category))
        for result_seq, counts, category in question_searches:
            counts[f"question_results_from_{category}"] += 1
            if any(result_seq < later for later in question_sequences):
                counts["question_results_followed_by_question_call"] += 1
                counts[f"followed_question_results_from_{category}"] += 1
    return matched, coverage, searches


def analyze(db):
    attempts, candidates = load_attempts(db)
    matched, coverage, searches = scan_messages(db, attempts, candidates)
    policies = load_latest_policy(db)
    totals = defaultdict(Counts)
    days = defaultdict(Counts)
    models = defaultdict(Counts)
    latest_policy = defaultdict(Counts)
    for identity, a in attempts.items():
        if a.role != "user":
            continue
        q, b = matched.get(identity, (0, 0))
        period = epoch(a.stamp)
        totals[a.route, period].add(a.transcript, q, b)
        days[a.route, a.stamp[:10]].add(a.transcript, q, b)
        models[a.route, period, a.model].add(a.transcript, q, b)
        if a.route == "codex" and period == "after":
            callable_deferred, advertised, search = policies.get(a.transcript, (False, False, False))
            state = "search_only" if callable_deferred and search and not advertised else (
                "search_and_advertised" if callable_deferred and search and advertised else "other"
            )
            latest_policy[state].add(a.transcript, q, b)
    parent_ledger = defaultdict(Counter)
    for stamp, route, kind, count in db.execute("""
        SELECT substr(u.created_at,1,10),p.route_class,p.kind,count(*)
        FROM turn_usage u JOIN conversations c ON c.id=u.conversation_id
        LEFT JOIN request_capacity_plans p ON p.plan_id=u.capacity_plan_id
        WHERE u.created_at >= ? AND c.runtime_role='user'
        GROUP BY 1,2,3
    """, (START,)):
        key = "unlinked" if kind is None else "ordinary" if kind == "ordinary" else "other_kind"
        parent_ledger[epoch(stamp)][key] += count
        if kind == "ordinary":
            parent_ledger[epoch(stamp)][ROUTES.get(route, "other_route")] += count
    return {
        "windows_utc": {"start": START, "cutover": CUTOVER, "latest_observed_request_day": max(
            (a.stamp[:10] for a in attempts.values()), default=None
        )},
        "parent_by_route": {route: {p: totals[route, p].report() for p in ("before", "after")}
                            for route in ("codex", "openai_responses", "anthropic", "other")},
        "parent_by_day": {route: {day: v.report() for (r, day), v in sorted(days.items()) if r == route}
                          for route in ("codex", "openai_responses", "anthropic", "other")},
        "codex_parent_by_model": {p: {m: counts.report()
                                    for (r, period, m), counts in sorted(models.items())
                                    if r == "codex" and period == p}
                                  for p in ("before", "after")},
        "codex_parent_latest_policy_after": {name: counts.report() for name, counts in sorted(latest_policy.items())},
        "parent_usage_ledger_coverage": {p: dict(counts) for p, counts in sorted(parent_ledger.items())},
        "attribution_coverage": {p: {role: dict(values) for (period, role), values in sorted(coverage.items())
                                     if period == p} for p in ("before", "after")},
        "search_by_route": {route: {p: {role: dict(counts)
                                       for (period, role), counts in sorted(breakdown.items()) if period == p}
                                    for p in ("before", "after")} for route, breakdown in sorted(searches.items())},
    }


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
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
