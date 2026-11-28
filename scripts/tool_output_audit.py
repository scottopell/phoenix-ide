#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.13"
# dependencies = []
# ///
"""Measure tool history carry and replay local clearing without exporting content."""

from __future__ import annotations

import argparse
from bisect import bisect_left
from collections import Counter, defaultdict
from contextlib import closing
from dataclasses import dataclass, field
from datetime import datetime
from hashlib import sha256
from itertools import groupby, pairwise
import json
from pathlib import Path
import posixpath
import re
import shlex
import sqlite3
from statistics import median
import sys
import zlib

IMAGE_TOKENS = 1500
DEFAULT_SLOPE = 0.25
KEEP_RECENT_ROUNDS = 3
CLEARABLE = frozenset("""
bash browser_recent_console_logs browser_take_screenshot keyword_search read_file read_image
search terminal_command_history terminal_last_command tmux tmux_run list_watched_conversations
previous_transcripts query_database read_conversation read_event_evidence read_goal prepare_goal
apply_goal resolve_reference search_conversations
""".split())
NATIVE = CLEARABLE | frozenset("""
answer_conversation_question ask_user_question browser_clear_console_logs browser_click
browser_eval browser_key_press browser_navigate browser_profile browser_resize browser_snapshot
browser_type browser_wait_for_selector close_conversation continue_conversation create_conversation
list_user_questions patch post_user_question present_svg propose_task reminders rename_conversation
send_conversation_message skill spawn_agents submit_error submit_result think tracking
unwatch_conversation watch_conversation withdraw_user_question work_scope_load tool_search
""".split())
SIMPLE_FAMILIES = frozenset("""
rg grep cat sed head tail ls find tree npm pnpm npx pytest python uv curl gh jq make docker
""".split())
SUBCOMMANDS = {
    "cargo": frozenset("build check test clippy nextest fmt run".split()),
    "git": frozenset("diff log show status grep".split()),
    "./dev.py": frozenset("check up restart".split()),
}
OPTIONS_WITH_VALUES = {
    "git": frozenset({"-C", "-c", "--config-env", "--git-dir", "--work-tree", "--namespace"}),
    "cargo": frozenset({"-Z", "--config", "--color", "--manifest-path"}),
    "./dev.py": frozenset(),
}
PIPE_FILTERS = frozenset("head tail grep rg wc jq".split())
READ_ONLY_BASH = SIMPLE_FAMILIES.intersection("rg grep cat head tail ls find tree jq".split()) | {
    "git diff", "git log", "git show", "git status", "git grep",
}
ANSI = re.compile(rb"\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07\x1b]*(?:\x07|\x1b\\)|[@-Z\\-_])")
BASE64_RUN = re.compile(rb"[A-Za-z0-9+/]{200,}={0,2}")
NUMBERED = re.compile(rb"^ *(\d+)\t")
CARGO_PROGRESS = re.compile(rb"^\s*(?:Compiling|Checking|Downloaded) ")
CARGO_TEST_OK = re.compile(rb"^\s*test .+ \.\.\. ok\s*$")
CARGO_WARNING = re.compile(rb"^\s*warning(?:\[[^\]]{1,12}\])?:")
CARGO_WARNING_CONT = re.compile(rb"^(?:\s+|\d+\s*\|)")
COMMAND_ASSIGNMENT = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*=")


def open_read_only(path: Path) -> sqlite3.Connection:
    db = sqlite3.connect(f"{path.resolve().as_uri()}?mode=ro", uri=True)
    db.execute("PRAGMA query_only = ON")
    db.execute("BEGIN")
    return db


def parse(value: str | None):
    if not value:
        return None
    try:
        return json.loads(value)
    except (ValueError, RecursionError):
        return None


def object_value(value) -> dict:
    return value if isinstance(value, dict) else {}


def compact(value) -> bytes:
    return json.dumps(value, separators=(",", ":"), sort_keys=True, ensure_ascii=False).encode()


def digest(value: bytes) -> bytes:
    return sha256(value).digest()


def value_size(value) -> int:
    return len(value.encode()) if isinstance(value, str) else len(compact(value))


def quantiles(values: list) -> dict:
    if not values:
        return {"p50": 0, "p90": 0, "p99": 0, "max": 0}
    ordered = sorted(values)
    return {f"p{p}": ordered[round((len(ordered) - 1) * p / 100)] for p in (50, 90, 99)} | {
        "max": ordered[-1]
    }


def usage_key(usage: dict) -> tuple[int, int, int, int]:
    fields = ("input_tokens", "cache_creation_tokens", "cache_read_tokens", "output_tokens")
    return tuple(int(usage.get(key) or 0) for key in fields)


def timestamp(value: str) -> float:
    try:
        return datetime.fromisoformat(value).timestamp()
    except ValueError:
        return 0.0


@dataclass(frozen=True, slots=True)
class BashShape:
    family: str = "other"
    filter: bool = False
    may_write: bool = True


def program_family(tokens: list[str]) -> str:
    while tokens and (COMMAND_ASSIGNMENT.match(tokens[0]) or tokens[0] in {"env", "command", "exec"}):
        tokens = tokens[1:]
    if not tokens:
        return "other"
    program = posixpath.basename(tokens[0])
    if tokens[0] == "./dev.py":
        program = "./dev.py"
    if program in SUBCOMMANDS:
        arguments = iter(tokens[1:])
        subcommand = ""
        for argument in arguments:
            if argument in OPTIONS_WITH_VALUES[program]:
                next(arguments, None)
            elif not argument.startswith("-") and not (program == "cargo" and argument.startswith("+")):
                subcommand = argument
                break
        return f"{program} {subcommand if subcommand in SUBCOMMANDS[program] else 'other'}"
    return program if program in SIMPLE_FAMILIES else "other"


def bash_shape(command: str) -> BashShape:
    lexer = shlex.shlex(command, posix=True, punctuation_chars="|&;()<>")
    lexer.whitespace_split = True
    lexer.commenters = ""
    try:
        tokens = list(lexer)
    except ValueError:
        return BashShape()
    segments: list[list[str]] = [[]]
    operators = []
    for token in tokens:
        if token and all(char in "|&;()<>" for char in token):
            operators.append(token)
            segments.append([])
        else:
            segments[-1].append(token)
    family = program_family(segments[0])
    pipe_count = next((i for i, op in enumerate(operators) if op != "|"), len(operators))
    pipe_filter = (
        pipe_count > 0
        and bool(segments[pipe_count])
        and posixpath.basename(segments[pipe_count][0]) in PIPE_FILTERS
    )
    read_only = (
        bool(tokens)
        and all(op == "|" for op in operators)
        and all(program_family(segment) in READ_ONLY_BASH for segment in segments)
        and not any(marker in command for marker in ("\n", "`", "$(", "-delete", "-exec", "--output"))
    )
    return BashShape(family, pipe_filter, not read_only)


def path_key(args: dict) -> bytes | None:
    path = args.get("path")
    if not isinstance(path, str) or not path:
        return None
    return digest(posixpath.normpath(path).encode())


def requested_limit(tool: str, args: dict) -> int | None:
    if tool == "bash" and args.get("since") is not None:
        return None
    key, default = {"read_file": ("limit", 2000), "bash": ("lines", 200),
                    "search": ("max_results", 50)}.get(tool, ("limit", None))
    value = args.get(key, default)
    return value if isinstance(value, int) and value > 0 else None


@dataclass(slots=True)
class Call:
    tool: str
    input_key: bytes
    input_size: int
    path: bytes | None
    limit: int | None
    scope: bytes | None = None
    bash: BashShape = BashShape()
    handle: str | None = None
    query: str = ""
    terms: frozenset[bytes] = frozenset()
    may_write: bool = False


def classify_call(tool: str, value, handles: dict[str, BashShape]) -> Call:
    args = object_value(value)
    encoded = compact(value)
    tool = tool if tool in NATIVE else "other_tool"
    call = Call(tool, digest(encoded), len(encoded), path_key(args), requested_limit(tool, args))
    if tool == "bash":
        cmd = args.get("cmd") or args.get("command")
        handle = args.get("handle")
        if isinstance(cmd, str) and cmd:
            call.bash = bash_shape(cmd)
            call.may_write = call.bash.may_write
        elif isinstance(handle, str) and handle:
            call.handle = handle
            call.bash = handles.get(handle, BashShape())
        target = args.get("target") or args.get("work_scope_id") or args.get("cwd")
        call.scope = digest(compact(target)) if target is not None else None
    if tool in {"keyword_search", "search"}:
        query = args.get("query") or args.get("pattern")
        call.query = query if isinstance(query, str) else ""
        terms = args.get("search_terms")
        if isinstance(terms, list):
            call.terms = frozenset(digest(term.encode()) for term in terms if isinstance(term, str))
    return call


@dataclass(slots=True)
class Result:
    seq: int
    round: int
    call: Call
    clearable: bool
    size: int
    marker_size: int
    images: int
    error: bool
    removed: dict[str, int]
    gzip_size: int
    line_count: int
    caps: tuple[str, ...]
    repeat: dict[str, int] = field(default_factory=dict)
    overlap_bytes: int = 0
    diff_bytes: int | None = None
    upper_carries: int = 0
    carries: int = 0


def inspect_output(
    data: bytes, tool: str, family: str, error: bool, limit: int | None, projected_lines: int | None,
    *, line_payload: bytes | None = None,
) -> tuple[dict[str, int], int, int, tuple[str, ...], list[tuple[int, bytes]], int | None]:
    removed = Counter()
    payload = data if line_payload is None else line_payload
    if b"\x1b" in payload:
        removed["ansi"] = sum(len(match[0]) for match in ANSI.finditer(payload))
    if data.lstrip()[:1] in (b"{", b"["):
        obj = parse(data)
        if obj is not None:
            removed["json_whitespace"] = max(0, len(data) - len(compact(obj)))
    for match in BASE64_RUN.finditer(data):
        # Preserve a short omission marker and the first few characters.
        removed["base64_like"] += len(match[0]) - 40

    if not data:
        return {}, 0, 0, (), [], None

    lines = payload.split(b"\n")
    if lines[-1] == b"":
        lines.pop()
    newline_count = len(lines) - 1 + payload.endswith(b"\n")
    read_lines = []
    bash_cargo = tool == "bash" and family.startswith("cargo ")
    passing = bash_cargo and family in {"cargo test", "cargo nextest"} and not error and b"test result: ok" in payload
    previous = None
    repeats = 0
    previous_blank = False
    in_warning = False
    max_line = 0
    for index, line in enumerate(lines):
        line_size = len(line)
        max_line = max(max_line, line_size)
        with_newline = line_size + (index < newline_count)
        if b"\r" in line:
            removed["progress_overwrite"] += max(0, line.rstrip(b"\r").rfind(b"\r") + 1)
        if line == previous:
            repeats += 1
        elif repeats:
            removed["duplicate_lines"] += max(0, (len(previous) + 1) * repeats - len(str(repeats + 1)) - 4)
            repeats = 0
        previous = line
        body = line.rstrip(b" \t\r")
        blank = not body
        if blank and previous_blank:
            removed["blank_lines"] += with_newline
        previous_blank = blank
        removed["trailing_whitespace"] += line_size - len(body)
        if line_size > 2000:
            characters = line.decode(errors="replace")
            if len(characters) > 2000:
                removed["long_lines"] += line_size - len(characters[:500].encode()) - 24
        if tool == "read_file":
            match = NUMBERED.match(line)
            if match:
                removed["read_prefix_all"] += match.end()
                removed["read_prefix_padding"] += match.end() - len(match[1]) - 1
                read_lines.append((int(match[1]), line[match.end():]))
        if bash_cargo:
            if CARGO_PROGRESS.match(line):
                removed["cargo_progress"] += with_newline
            if CARGO_WARNING.match(line) or (in_warning and CARGO_WARNING_CONT.match(line)):
                removed["cargo_warnings"] += with_newline
                in_warning = True
            else:
                in_warning = False
            if passing and CARGO_TEST_OK.match(line):
                removed["passing_test_lines"] += with_newline
    if repeats:
        removed["duplicate_lines"] += max(
            0, (len(previous) + 1) * repeats - (not payload.endswith(b"\n")) - len(str(repeats + 1)) - 4
        )

    caps = []
    size = len(data)
    if size >= 100 * 1024 * 0.95 or b"[truncated " in data and b"bytes of" in data:
        caps.append("runtime_result_100k")
    if tool == "read_file" and limit and len(read_lines) >= 0.95 * limit:
        caps.append("read_file_2000" if limit == 2000 else "read_file_explicit")
    if tool == "keyword_search" and size >= 128 * 1024 * 0.95:
        caps.append("keyword_search_128k")
    if tool in {"tmux", "tmux_run"} and size >= 128 * 1024 * 0.95:
        caps.append("tmux_128k")
    if tool == "bash":
        observed_lines = projected_lines if projected_lines is not None else len(lines)
        if limit and observed_lines >= limit * 0.95:
            caps.append("bash_tail_200" if limit == 200 else "bash_tail_explicit")
        if max_line >= 64 * 1024 * 0.95:
            caps.append("bash_partial_64k_proxy")
    diff_size = None
    if tool == "patch":
        diff = data.partition(b"<diff>")
        if diff[1]:
            diff_body = diff[2].partition(b"</diff>")[0].strip(b"\n")
            diff_size = len(diff_body)
            if diff_size >= 16 * 1024 * 0.95:
                caps.append("patch_diff_16k")
            if b"... diff truncated:" in diff_body:
                caps.append("patch_diff_truncated")
    # These candidate caps retain a fixed omission marker.
    if tool in {"bash", "read_file", "keyword_search", "tmux", "tmux_run", "read_conversation",
                "query_database", "spawn_agents", "search"}:
        for kib in (8, 16, 32):
            removed[f"cap_{kib}k"] = max(0, size - kib * 1024 - 48)
    return ({key: val for key, val in removed.items() if val > 0},
            len(zlib.compress(data, level=1, wbits=31)), len(lines), tuple(caps), read_lines, diff_size)


@dataclass(frozen=True, slots=True)
class Request:
    seq: int
    total: int
    prefix_bytes: int
    prefix_images: int
    result_round: int
    reduction: int | None


@dataclass(slots=True)
class Transcript:
    role: str
    watermark: int
    requests: list[Request] = field(default_factory=list)
    results: list[Result] = field(default_factory=list)
    assistant: list[tuple[int, str, int]] = field(default_factory=list)
    calls: dict[str, Call] = field(default_factory=dict)
    handles: dict[str, BashShape] = field(default_factory=dict)
    seen_outputs: set[tuple] = field(default_factory=set)
    failed_inputs: set[tuple] = field(default_factory=set)
    latest_reads: dict[bytes, dict[int, bytes]] = field(default_factory=dict)
    input_bytes: int = 0
    input_images: int = 0
    call_round: int = -1
    result_round: int = 0
    observations: Counter = field(default_factory=Counter)

    def add(self, row, hints: dict[tuple, list[tuple[float, int]]], image_counts: dict[str, int]):
        _, mid, seq, kind, raw, usage, created_at = row
        if kind in {"system", "error"}:
            return
        data = parse(raw)
        if data is None:
            self.observations["invalid_content"] += 1
            return
        if kind == "agent":
            self.add_assistant(seq, raw, data, usage, created_at, hints)
        elif kind == "tool":
            self.add_result(mid, seq, object_value(data), image_counts)
        else:
            body = object_value(data)
            if kind == "skill":
                self.input_bytes += value_size(body.get("body", "")) + 60
            elif kind == "continuation":
                self.input_bytes += value_size(body.get("summary", "")) + 1400
            else:
                rendered = body.get("llm_text")
                text = body.get("text", "") if rendered is None else rendered
                self.input_bytes += value_size(text) + 60
                self.input_images += max(image_counts.get(mid, 0), len(body.get("images") or []))

    def add_assistant(self, seq, raw, data, usage, created_at, hints):
        accounting = object_value(parse(usage))
        if accounting:
            usage_values = usage_key(accounting)
            total = sum(usage_values[:3])
            if total > 0:
                tagged = hints.get(usage_values, ())
                now = timestamp(created_at) if tagged else 0
                nearby = [reduction for when, reduction in tagged if abs(when - now) <= 120]
                reduction = nearby[0] if len(nearby) == 1 else None
                self.observations["matched_anchors"] += reduction is not None
                self.requests.append(Request(seq, total, self.input_bytes, self.input_images,
                                             self.result_round, reduction))
        blocks = data if isinstance(data, list) else []
        started_round = False
        accounted = 0
        for raw_block in blocks:
            block = object_value(raw_block)
            kind = block.get("type")
            if kind == "tool_use":
                if not started_round:
                    self.call_round += 1
                    started_round = True
                call = classify_call(block.get("name", ""), block.get("input"), self.handles)
                self.calls[block.get("id", "")] = call
                key, size = f"tool_input {call.tool}", call.input_size
                self.observations["tool_uses"] += 1
            elif kind == "text":
                key, size = "assistant_text", value_size(block.get("text", ""))
            elif kind in {"thinking", "reasoning", "redacted_thinking"}:
                key = "assistant_reasoning"
                size = value_size(block.get("thinking", block.get("text", block.get("data", ""))))
            else:
                key, size = "assistant_other", len(compact(block))
            accounted += size
            self.assistant.append((seq, key, size))
        stored_bytes = len(raw.encode())
        if stored_bytes > accounted:
            self.assistant.append((seq, "assistant_serialization", stored_bytes - accounted))
        self.input_bytes += stored_bytes

    def add_result(self, mid, seq, body, image_counts):
        call = self.calls.get(body.get("tool_use_id"))
        if call is None:
            self.observations["unpaired_results"] += 1
            call = Call("unpaired_tool", b"", 0, None, None)
        tool = call.tool
        raw = body.get("content") or ""
        if not isinstance(raw, str):
            raw = compact(raw).decode()
        data = raw.encode()
        error = bool(body.get("is_error"))
        bash_data = object_value(parse(raw)) if tool == "bash" else {}
        if tool == "bash":
            # Legacy result payloads can supply the command when a handle was
            # returned before the retained transcript begins.
            handle = bash_data.get("handle")
            command = bash_data.get("cmd")
            if call.handle and isinstance(command, str) and call.handle not in self.handles:
                call.bash = bash_shape(command)
            if isinstance(handle, str):
                self.handles[handle] = call.bash
        lines = bash_data.get("lines")
        line_data = ([line if isinstance(line, str) else object_value(line).get("bytes") for line in lines]
                     if isinstance(lines, list) else None)
        if line_data is not None:
            line_data = [line for line in line_data if isinstance(line, str)]
        projected_lines = len(line_data) if line_data is not None else None
        payload = "\n".join(line_data).encode() if line_data is not None else None
        analysis_error = error or bash_data.get("exit_code") not in (None, 0)
        removed, gzip_size, line_count, caps, read_lines, diff_bytes = inspect_output(
            data, tool, call.bash.family, analysis_error, call.limit, projected_lines, line_payload=payload)
        if tool == "bash" and isinstance(bash_data.get("cmd"), str):
            removed["bash_command_echo"] = len(compact(bash_data["cmd"])) + len(',"cmd":')
        if b"[results truncated: search scope" in data and tool == "keyword_search":
            caps += ("keyword_search_scope_truncated",)
        if b"[raw output shortened for display]" in data and tool == "keyword_search":
            caps += ("keyword_search_fallback_8k",)

        images = max(len(body.get("images") or []), image_counts.get(mid, 0))
        marker = 113 + len(mid) + len(str(seq))
        item = Result(seq, max(0, self.call_round), call,
                      tool in CLEARABLE or tool == "skill" and self.role == "coordinator",
                      len(data), marker, images, error, removed, gzip_size,
                      projected_lines if projected_lines is not None else line_count, caps, diff_bytes=diff_bytes)
        self.result_round = max(self.result_round, item.round)
        self.input_bytes += item.size
        self.input_images += images
        self.results.append(item)

        if tool == "patch" and not error:
            if call.path is None:
                self.latest_reads.clear()
            else:
                self.latest_reads.pop(call.path, None)
        if call.may_write or tool in {"tmux", "tmux_run", "browser_eval", "other_tool", "unpaired_tool"}:
            self.latest_reads.clear()
        if tool == "read_file" and call.path is not None and read_lines and not error:
            previous = self.latest_reads.get(call.path, {})
            current = {line: digest(text) for line, text in read_lines}
            item.overlap_bytes = sum(len(text) + 1 for line, text in read_lines
                                     if previous.get(line) == current[line])
            self.latest_reads[call.path] = current

        if call.input_key:
            output_key = digest(data)
            repeat_key = (tool, call.input_key, output_key, images, error)
            if data and not images and repeat_key in self.seen_outputs:
                item.repeat["identical_call_and_output"] = item.size
            self.seen_outputs.add(repeat_key)
            if (tool == "bash" and not analysis_error and not call.may_write and line_data
                    and call.bash.family in {"git status", "git diff"}):
                git_output = compact([line_data, bash_data.get("partial"), bash_data.get("truncated_before")])
                git_key = (call.bash.family, call.scope, digest(git_output))
                if git_key in self.seen_outputs:
                    item.repeat["repeated_git_status_or_diff"] = min(len(git_output), item.size)
                self.seen_outputs.add(git_key)
            failed_key = (tool, call.input_key)
            if failed_key in self.failed_inputs:
                item.repeat["retry_after_failing_input"] = item.size
                if error:
                    item.repeat["identical_failing_input"] = item.size
            if error:
                self.failed_inputs.add(failed_key)
            else:
                self.failed_inputs.discard(failed_key)


def calibrate(requests: list[Request]) -> tuple[float, int]:
    slopes = []
    for prior, req in pairwise(requests):
        added = req.prefix_bytes - prior.prefix_bytes
        increase = req.total - prior.total - (req.prefix_images - prior.prefix_images) * IMAGE_TOKENS
        if req.reduction in (0, None) and added >= 512 and 0.10 <= increase / added <= 0.55:
            slopes.append(increase / added)
    if len(slopes) < 6:
        return DEFAULT_SLOPE, len(slopes)
    # Use a fixed small prior so short but technically eligible transcripts
    # cannot swing the cohort estimate on a few request transitions.
    return median(slopes + [DEFAULT_SLOPE] * 4), len(slopes)


@dataclass(slots=True)
class Reconstruction:
    counts: Counter = field(default_factory=Counter)
    residuals: list[float] = field(default_factory=list)
    annotated_residuals: list[float] = field(default_factory=list)
    slopes: list[float] = field(default_factory=list)


def reconstruct(transcript: Transcript, slope: float, diagnostics: Reconstruction):
    requests = transcript.requests
    seqs = [r.seq for r in requests]
    candidates = [r for r in transcript.results if r.clearable and r.seq <= transcript.watermark]
    for item in transcript.results:
        item.upper_carries = len(seqs) - bisect_left(seqs, item.seq + 1)
        item.carries = item.upper_carries
    cursor = 0
    for i, request in enumerate(requests):
        prev = requests[i - 1] if i else None
        drop = prev.total - request.total if prev else 0
        expected = (request.prefix_bytes - prev.prefix_bytes) * slope + (
            request.prefix_images - prev.prefix_images) * IMAGE_TOKENS if prev else 0
        target = drop + expected
        if drop > 0:
            diagnostics.counts["observed_drops"] += 1
            diagnostics.counts["observed_drop_tokens"] += drop
        hint = request.reduction or 0
        if hint:
            diagnostics.counts["tagged_requests"] += 1
        unexplained = (request.reduction is None and prev is not None and
                       (drop > 0 and target >= 3000 or target >= max(8192, 0.05 * prev.total)))
        freed = 0.0
        if cursor < len(candidates) and (hint or unexplained):
            stop = cursor
            cumulative = []
            provider_freed = []
            provider_total = estimated_total = 0.0
            while stop < len(candidates):
                item = candidates[stop]
                if item.seq >= request.seq or item.round > request.result_round - KEEP_RECENT_ROUNDS:
                    break
                size_difference = item.size - item.marker_size
                provider_total += size_difference * slope + item.images * IMAGE_TOKENS
                estimated_total += size_difference * DEFAULT_SLOPE + item.images * IMAGE_TOKENS
                cumulative.append(estimated_total if hint else provider_total)
                provider_freed.append(provider_total)
                stop += 1
            if cumulative:
                expected_reduction = hint or target
                index = min(range(len(cumulative)), key=lambda j: abs(cumulative[j] - expected_reduction))
                residual = abs(cumulative[index] - expected_reduction)
                tolerance = max(1500, expected_reduction * (0.50 if hint else 0.33))
                if cumulative[index] > 0 and residual <= tolerance:
                    freed = provider_freed[index]
                    end = cursor + index + 1
                    for item in candidates[cursor:end]:
                        item.carries -= len(requests) - i
                    cursor = end
                    diagnostics.counts["tagged_sweeps" if hint else "inferred_sweeps"] += 1
                    if hint:
                        diagnostics.annotated_residuals.append(residual)
        if drop > 0:
            residual = abs(max(0, target) - freed)
            diagnostics.residuals.append(residual)
            if freed and residual <= max(2000, 0.33 * max(drop, target)):
                diagnostics.counts["explained_drops"] += 1
                diagnostics.counts["explained_drop_tokens"] += drop
    diagnostics.counts["eligible_by_final_watermark"] += len(candidates)
    diagnostics.counts["assigned_clear_time"] += cursor
    diagnostics.counts["still_unassigned_full_carry_tokens"] += sum(
        (item.size * slope + item.images * IMAGE_TOKENS) * item.carries for item in candidates[cursor:]
    )


@dataclass(slots=True)
class Inventory:
    counts: Counter = field(default_factory=Counter)
    sizes: list[int] = field(default_factory=list)
    tokens: list[float] = field(default_factory=list)
    transforms: dict[str, Counter] = field(default_factory=lambda: defaultdict(Counter))
    redundant: dict[str, Counter] = field(default_factory=lambda: defaultdict(Counter))

    def add(self, item: Result, slope: float):
        tokens = item.size * slope + item.images * IMAGE_TOKENS
        self.sizes.append(item.size)
        self.tokens.append(tokens)
        counts = self.counts
        counts["calls"] += 1
        counts["errors"] += item.error
        counts["images"] += item.images
        counts["image_results"] += bool(item.images)
        counts["clearable_results"] += item.clearable
        counts["cleared_marker_turns"] += item.upper_carries - item.carries
        counts["clearable_carry"] += tokens * item.carries if item.clearable else 0
        counts["nonclearable_carry"] += tokens * item.carries if not item.clearable else 0
        counts["carry"] += tokens * item.carries
        counts["upper"] += tokens * item.upper_carries
        counts["uncleared_marker_carry"] += (item.upper_carries - item.carries) * item.marker_size * slope
        counts["bytes"] += item.size
        counts["tokens"] += tokens
        counts["gzip_bytes"] += item.gzip_size
        counts["pipe_filter_calls"] += item.call.bash.filter
        if item.error:
            counts["error_bytes"] += item.size
            counts["error_tokens"] += tokens
            counts["error_carry"] += tokens * item.carries
            counts["error_upper"] += tokens * item.upper_carries
        for label, size in item.removed.items():
            if size:
                self.change(self.transforms[label], size, size * slope, item)
        for label, size in item.repeat.items():
            self.change(self.redundant[label], size, size * slope, item)
        if item.overlap_bytes:
            self.change(self.redundant["overlapping_unchanged_read"], item.overlap_bytes,
                        item.overlap_bytes * slope, item)

    @staticmethod
    def change(group, size, tokens, item):
        group["results"] += 1
        group["bytes"] += size
        group["carry"] += tokens * item.carries
        group["upper"] += tokens * item.upper_carries

    def report(self, total_tokens, ledger):
        return {
            **self.counts, "bytes_distribution": quantiles(self.sizes), "token_distribution": quantiles(self.tokens),
            "tool_output_token_share": ratio(self.counts["tokens"], total_tokens),
            "gzip_ratio": ratio(self.counts["gzip_bytes"], self.counts["bytes"]),
            "error_rate": ratio(self.counts["errors"], self.counts["calls"]),
            "clearability": (
                "mixed" if 0 < self.counts["clearable_results"] < self.counts["calls"]
                else "clearable" if self.counts["clearable_results"]
                else "unknown" if self.counts["unknown_tool_calls"]
                else "nonclearable"
            ),
            "carry_share_of_recorded_input": ratio(self.counts["carry"], ledger),
            "upper_share_of_recorded_input": ratio(self.counts["upper"], ledger),
            "transforms": {
                label: {**nums, "byte_share": ratio(nums["bytes"], self.counts["bytes"]),
                        "share_of_recorded_input": ratio(nums["carry"], ledger),
                        "upper_share_of_recorded_input": ratio(nums["upper"], ledger)}
                for label, nums in sorted(self.transforms.items())
            },
            "redundancy": {
                label: {**nums, "share_of_recorded_input": ratio(nums["carry"], ledger),
                        "upper_share_of_recorded_input": ratio(nums["upper"], ledger)}
                for label, nums in sorted(self.redundant.items())
            }
        }


def ratio(numerator, denominator):
    return numerator / denominator if denominator else 0.0


def narrowed(previous: Call, current: Call) -> bool:
    same_path = previous.path == current.path
    smaller_limit = same_path and previous.limit is not None and current.limit is not None and (
        current.limit < previous.limit)
    more_specific = (
        bool(previous.query)
        and previous.query in current.query
        and len(current.query) > len(previous.query)
    ) or (bool(current.terms) and current.terms < previous.terms)
    new_filter = current.bash.filter and not previous.bash.filter
    return smaller_limit or more_specific or new_filter


@dataclass(slots=True)
class Audit:
    diagnostic: Reconstruction = field(default_factory=Reconstruction)
    tools: dict[str, Inventory] = field(default_factory=lambda: defaultdict(Inventory))
    families: dict[str, Inventory] = field(default_factory=lambda: defaultdict(Inventory))
    caps: dict[str, Counter] = field(default_factory=lambda: defaultdict(Counter))
    redundancy: dict[str, Counter] = field(default_factory=lambda: defaultdict(Counter))
    assistant: dict[str, Counter] = field(default_factory=lambda: defaultdict(Counter))
    patch_input: list[float] = field(default_factory=list)
    patch_diff: list[float] = field(default_factory=list)
    patch_ratios: list[float] = field(default_factory=list)
    observations: Counter = field(default_factory=Counter)
    surface_coverage: dict[str, Counter] = field(default_factory=lambda: defaultdict(Counter))

    def add(self, transcript: Transcript):
        slope, samples = calibrate(transcript.requests)
        if samples >= 6:
            self.diagnostic.slopes.append(slope)
        self.observations.update(transcript.observations)
        self.observations["transcripts"] += 1
        self.observations["message_requests"] += len(transcript.requests)
        self.observations["message_request_input"] += sum(r.total for r in transcript.requests)
        reconstruct(transcript, slope, self.diagnostic)
        seqs = [req.seq for req in transcript.requests]

        surfaces = defaultdict(set)
        for seq, label, size in transcript.assistant:
            carries = len(seqs) - bisect_left(seqs, seq + 1)
            self.assistant[label].update({"blocks": 1, "bytes": size, "tokens": size * slope,
                                          "carry": size * slope * carries})
            if label.startswith("tool_input browser_"):
                surfaces["browser"].add(seq)
            elif label in {"tool_input tmux", "tool_input tmux_run"}:
                surfaces["tmux"].add(seq)
            elif label in {"tool_input present_svg", "tool_input spawn_agents"}:
                surfaces[label.removeprefix("tool_input ")].add(seq)
        for surface, calls in surfaces.items():
            self.surface_coverage[surface].update({
                "transcripts": 1,
                "call_bearing_recorded_requests": sum(seq in calls for seq in seqs),
                "recorded_requests_in_calling_transcripts": len(seqs),
                "recorded_requests_through_first_call": bisect_left(seqs, min(calls) + 1),
            })

        prior_results = {}
        for item in transcript.results:
            call, tool = item.call, item.call.tool
            self.tools[tool].add(item, slope)
            if tool in {"other_tool", "unpaired_tool"}:
                self.tools[tool].counts["unknown_tool_calls"] += 1
            if tool == "patch" and item.diff_bytes is not None:
                self.patch_input.append(call.input_size * slope)
                self.patch_diff.append(item.diff_bytes * slope)
                if item.diff_bytes > 0:
                    self.patch_ratios.append(call.input_size / item.diff_bytes)
            family = call.bash.family if tool == "bash" else ""
            if tool == "bash":
                self.families[family].add(item, slope)
            key = tool, family
            prior = prior_results.get(key)
            if prior is not None:
                for label in prior.caps:
                    stats = self.caps[label]
                    stats["followed"] += 1
                    stats["narrowed"] += narrowed(prior.call, call)
            for label in item.caps:
                self.caps[label]["hits"] += 1
            prior_results[key] = item
            for label, size in item.repeat.items():
                Inventory.change(self.redundancy[label], size, size * slope, item)
            if item.overlap_bytes:
                Inventory.change(self.redundancy["overlapping_unchanged_read"], item.overlap_bytes,
                                 item.overlap_bytes * slope, item)

    def report(self, billing, limited):
        ledger = sum(billing[1:])
        exact_input = dict(zip(("attempt_rows", "fresh", "cache_write", "cache_read"), billing))
        total_tool_tokens = sum(tool.counts["tokens"] for tool in self.tools.values())
        order = lambda inventory: sorted(inventory.items(), key=lambda kv: -kv[1].counts["carry"])
        tools = {label: inv.report(total_tool_tokens, ledger) for label, inv in order(self.tools)}
        bash = {label: inv.report(self.tools["bash"].counts["tokens"], ledger)
                for label, inv in order(self.families)}
        assistant = {label: {**stats, "share_of_recorded_input": ratio(stats["carry"], ledger)}
                     for label, stats in sorted(self.assistant.items(), key=lambda kv: -kv[1]["carry"])}
        calibration = self.diagnostic.slopes
        return {
            "ledger": {**exact_input, "recorded_input": ledger},
            "partial_run": limited,
            "corpus": dict(self.observations),
            "clearing": {
                **self.diagnostic.counts, "prompt_drop_residual_tokens": quantiles(self.diagnostic.residuals),
                "tagged_reduction_residual_tokens": quantiles(self.diagnostic.annotated_residuals),
                "calibrated_transcripts": len(calibration),
                "median_tokens_per_byte": median(calibration) if calibration else DEFAULT_SLOPE,
                "slope_distribution": quantiles(calibration),
            },
            "tool_totals": {
                "output_tokens": total_tool_tokens,
                "reconstructed_carry": sum(row["carry"] for row in tools.values()),
                "upper_bound_carry": sum(row["upper"] for row in tools.values()),
                "clearable_reconstructed_carry": sum(row["clearable_carry"] for row in tools.values()),
                "nonclearable_reconstructed_carry": sum(row["nonclearable_carry"] for row in tools.values()),
                "estimated_marker_carry": sum(row["uncleared_marker_carry"] for row in tools.values()),
            },
            "tools": tools, "bash_families": bash,
            "caps": {name: {**counts, "narrow_after_rate": ratio(counts["narrowed"], counts["followed"])}
                     for name, counts in sorted(self.caps.items())},
            "redundancy": dict(self.redundancy),
            "assistant": assistant,
            "tool_surface_coverage": dict(self.surface_coverage),
            "patch_pairs": {
                "count": len(self.patch_input), "input_tokens": quantiles(self.patch_input),
                "diff_tokens": quantiles(self.patch_diff), "input_to_diff_ratio": quantiles(self.patch_ratios),
            },
        }


def analyze(db: sqlite3.Connection, limit: int | None) -> dict:
    billing = db.execute(
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
        anchors[conv][(fresh, write, read, output)].append((timestamp(created), reduction))
    rows = db.execute(
        """SELECT conversation_id, message_id, sequence_id, message_type, content, usage_data, created_at
           FROM messages ORDER BY conversation_id, sequence_id"""
    )
    audit = Audit()
    audit.observations["ledger_capacity_anchors"] = sum(len(v) for groups in anchors.values() for v in groups.values())
    for index, (conv, group) in enumerate(groupby(rows, key=lambda row: row[0])):
        if limit is not None and index >= limit:
            break
        if conv not in convs:
            continue
        transcript = Transcript(*convs[conv])
        hints = anchors.get(conv, {})
        for row in group:
            transcript.add(row, hints, images)
        audit.add(transcript)
        if (index + 1) % 2500 == 0:
            print(f"Scanned {index + 1} transcripts.", file=sys.stderr)
    return audit.report(billing, limit is not None)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("database", type=Path)
    parser.add_argument("--json", type=Path, help="write to a new aggregate-only file")
    parser.add_argument("--limit-transcripts", type=int, help="partial scan for script validation")
    args = parser.parse_args()
    if args.limit_transcripts is not None and args.limit_transcripts < 1:
        parser.error("the validation transcript limit must be positive")
    if args.json and args.json.exists():
        parser.error("the JSON destination must be a new file")
    try:
        with closing(open_read_only(args.database.expanduser())) as db:
            payload = analyze(db, args.limit_transcripts)
    except (sqlite3.Error, ValueError, TypeError, OverflowError, RecursionError):
        print("Audit failed while reading or decoding the database.", file=sys.stderr)
        return 1
    encoded = json.dumps(payload, indent=2, allow_nan=False) + "\n"
    if args.json:
        with args.json.open("x", encoding="utf-8") as output:
            output.write(encoded)
    else:
        print(encoded, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
