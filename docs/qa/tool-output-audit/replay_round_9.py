#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.13"
# dependencies = ["tiktoken==0.14.0"]
# ///
"""Measure numbered file-read prompt savings with an offline BPE vocabulary."""

from bisect import bisect_right
from collections import Counter, defaultdict
from contextlib import closing
from dataclasses import dataclass, field
from hashlib import sha256
from itertools import groupby
import argparse
import json
import math
import os
from pathlib import Path
import random
import re
import sys

import tiktoken

from replay_round_2 import ReplayTranscript, baseline
from replay_round_3 import RUNTIME_TRUNCATION


VOCABULARY_KEY = "fb374d419588a4632f3f557e76b4b70aebbca790"
VOCABULARY_SHA256 = "446a9538cb6c348e3516120d7c08b09f57c36495e2acfffe59a5bf8b0cfb1a2d"
NUMBERED = re.compile(r"^( {0,5})([0-9]+)\t", re.M)
INTEGER = re.compile(r"(?<![\w.])[1-9][0-9]{0,7}(?![\w.])")
NAMED_LINE = re.compile(r"\blines?\s+(?:#\s*)?([1-9][0-9]{0,7})(?:\s*(?:[-–—,]|to)\s*([1-9][0-9]{0,7}))?", re.I)
PATH_LINE = re.compile(r"(?<!\w)([\w.~/-]+\.[\w-]+):([1-9][0-9]{0,7})(?!\d)")
SED_RANGE = re.compile(r"\bsed\s+[^\n]*?-n\s+['\"]?([1-9][0-9]{0,7})(?:,([1-9][0-9]{0,7}))?p\b")
PATCH_RANGE = re.compile(r"^@@ -([1-9][0-9]{0,7})(?:,\d+)? \+([1-9][0-9]{0,7})(?:,\d+)? @@", re.M)
LINE_FIELDS = {"offset", "line", "line_number", "start_line", "end_line", "line_start", "line_end",
               "from_line", "to_line"}
SIZE_LIMITS = (1024, 4096, 16384, 32768, 65536)
SIZE_LABELS = ("under_1k", "1k_to_4k", "4k_to_16k", "16k_to_32k", "32k_to_64k", "64k_or_more")
SAMPLE_PER_STRATUM = 1500


def offline_encoding(cache_dir):
    root = cache_dir.resolve()
    vocabulary = root / VOCABULARY_KEY
    if not vocabulary.is_file() or sha256(vocabulary.read_bytes()).hexdigest() != VOCABULARY_SHA256:
        raise SystemExit("The verified o200k_base vocabulary is required in the offline tokenizer cache.")
    os.environ["TIKTOKEN_CACHE_DIR"] = str(root)
    return tiktoken.get_encoding("o200k_base")


def source_range(value):
    if isinstance(value, bool):
        return None
    text = str(value)
    return int(text) if text.isascii() and text.isdecimal() and 1 <= len(text) <= 8 and int(text) > 0 else None


def numbers(value):
    return {int(match.group(0)) for match in INTEGER.finditer(value)}


def explicit_lines(value):
    patterns = (NAMED_LINE, SED_RANGE, PATCH_RANGE)
    tagged = {int(group) for pattern in patterns for match in pattern.finditer(value)
              for group in match.groups() if group}
    tagged.update(int(match.group(2)) for match in PATH_LINE.finditer(value))
    return tagged


def argument_parts(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, (int, float)) and not isinstance(value, bool):
        yield str(value)
    elif isinstance(value, dict):
        for child in value.values():
            yield from argument_parts(child)
    elif isinstance(value, list):
        for child in value:
            yield from argument_parts(child)


@dataclass(frozen=True, slots=True)
class Followup:
    broad: frozenset[int] = frozenset()
    explicit: frozenset[int] = frozenset()
    direct_file_offsets: tuple[tuple[bytes, int], ...] = ()


def next_evidence(parts, typed=(), files=()):
    value = "\n".join(parts)
    return Followup(frozenset(numbers(value)), frozenset(explicit_lines(value).union(typed)), tuple(files))


@dataclass(frozen=True, slots=True)
class NumberedRead:
    text: str
    spans: tuple[tuple[int, int], ...]
    numbered: int
    padding: int
    padding_bytes: int
    clipped: bool

    def contains(self, value):
        return any(a <= value <= b for a, b in self.spans)

    def transforms(self):
        if self.clipped or len(self.spans) != 1:
            return None
        without_padding = NUMBERED.sub(r"\2\t", self.text)
        first, last = self.spans[0]
        range_header = f"lines {first}–{last}\n"
        range_only = range_header + NUMBERED.sub("", self.text)
        return without_padding, range_only


def numbered_read(text):
    if not isinstance(text, str):
        return None
    spans = []
    pad_runs = pad_bytes = count = 0
    for line in NUMBERED.finditer(text):
        padding, raw_number = line.groups()
        number = int(raw_number)
        if number < 1 or len(padding) != max(0, 6 - len(raw_number)) or raw_number.startswith("0"):
            return None
        if spans and number == spans[-1][1] + 1:
            spans[-1] = spans[-1][0], number
        else:
            spans.append((number, number))
        count += 1
        pad_runs += bool(padding)
        pad_bytes += len(padding)
    if not spans:
        return None
    return NumberedRead(text, tuple(spans), count, pad_runs, pad_bytes, bool(RUNTIME_TRUNCATION.search(text)))


@dataclass(slots=True)
class Transcript(ReplayTranscript):
    reads: dict[int, NumberedRead] = field(default_factory=dict)
    prose: dict[int, Followup] = field(default_factory=dict)
    tools: dict[int, Followup] = field(default_factory=dict)
    boundaries: list[int] = field(default_factory=list)
    omitted: Counter = field(default_factory=Counter)

    def add(self, row, hints, images):
        _, _, seq, kind, _, _, _ = row
        if kind in {"user", "continuation"}:
            self.boundaries.append(seq)
        baseline.Transcript.add(self, row, hints, images)

    def add_assistant(self, seq, raw, data, usage, created_at, hints):
        ReplayTranscript.add_assistant(self, seq, raw, data, usage, created_at, hints)
        prose = []
        arguments = []
        line_fields = set()
        same_file = []
        saw_tool = False
        for block in data if isinstance(data, list) else ():
            if not isinstance(block, dict):
                continue
            kind = block.get("type")
            if kind == "text" and isinstance(block.get("text"), str):
                prose.append(block["text"])
            if kind != "tool_use":
                continue
            saw_tool = True
            args = baseline.object_value(block.get("input"))
            arguments.extend(argument_parts(args))
            for key in LINE_FIELDS & args.keys():
                if (number := source_range(args[key])) is not None:
                    line_fields.add(number)
            if block.get("name") == "read_file":
                path = baseline.path_key(args)
                offset = source_range(args.get("offset"))
                if path is not None and offset is not None:
                    same_file.append((path, offset))
        self.prose[seq] = next_evidence(prose)
        if saw_tool:
            self.tools[seq] = next_evidence(arguments, line_fields, same_file)

    def add_result(self, mid, seq, body, images):
        ReplayTranscript.add_result(self, mid, seq, body, images)
        item = self.results[-1]
        if item.call.tool != "read_file":
            return
        if item.error:
            self.omitted["errors"] += 1
            return
        if (result := numbered_read(body.get("content"))) is not None:
            self.reads[seq] = result
        else:
            self.omitted["without_recognized_numbered_lines"] += 1


@dataclass(frozen=True, slots=True)
class Sample:
    read: NumberedRead
    carries: int
    upper_carries: int


@dataclass(slots=True)
class Audit:
    totals: Counter = field(default_factory=Counter)
    risk: Counter = field(default_factory=Counter)
    strata: dict[str, Counter] = field(default_factory=lambda: defaultdict(Counter))
    samples: dict[str, list[Sample]] = field(default_factory=lambda: defaultdict(list))
    clearing: baseline.Reconstruction = field(default_factory=baseline.Reconstruction)
    rng: random.Random = field(default_factory=lambda: random.Random(921))

    def add(self, transcript):
        self.totals.update(transcript.omitted)
        if not transcript.reads:
            return
        slope, _ = baseline.calibrate(transcript.requests)
        baseline.reconstruct(transcript, slope, self.clearing)
        prose = list(transcript.prose)
        calls = list(transcript.tools)
        for item in transcript.results:
            read = transcript.reads.get(item.seq)
            if read is None:
                continue
            self.assess_risk(item, read, transcript, prose, calls)
            if read.clipped or len(read.spans) != 1:
                self.totals["clipped_or_discontinuous"] += 1
                self.totals["excluded_numbered_line_carries"] += read.numbered * item.carries
                self.totals["excluded_padding_run_carries"] += read.padding * item.carries
                self.totals["excluded_padding_bytes_carried"] += read.padding_bytes * item.carries
                continue
            label = SIZE_LABELS[bisect_right(SIZE_LIMITS, item.size)]
            stats = self.strata[label]
            stats.update({
                "population": 1,
                "rows_with_full_result_carry": item.carries > 0,
                "numbered_lines": read.numbered,
                "numbered_line_carries": read.numbered * item.carries,
                "upper_numbered_line_carries": read.numbered * item.upper_carries,
                "padding_run_carries": read.padding * item.carries,
                "padding_bytes_carried": read.padding_bytes * item.carries,
            })
            sample = self.samples[label]
            if len(sample) < SAMPLE_PER_STRATUM:
                sample.append(Sample(read, item.carries, item.upper_carries))
            elif (position := self.rng.randrange(stats["population"])) < SAMPLE_PER_STRATUM:
                sample[position] = Sample(read, item.carries, item.upper_carries)

    def assess_risk(self, item, read, transcript, prose_seqs, call_seqs):
        if not item.carries:
            self.risk["numbered_reads_without_full_carries"] += 1
            return
        stop_index = bisect_right(transcript.boundaries, item.seq)
        before = transcript.boundaries[stop_index] if stop_index < len(transcript.boundaries) else math.inf
        prose_index = bisect_right(prose_seqs, item.seq)
        call_index = bisect_right(call_seqs, item.seq)
        has_prose = prose_index < len(prose_seqs) and prose_seqs[prose_index] < before
        has_call = call_index < len(call_seqs) and call_seqs[call_index] < before
        prose = transcript.prose[prose_seqs[prose_index]] if has_prose else Followup()
        tool = transcript.tools[call_seqs[call_index]] if has_call else Followup()
        broad = any(read.contains(n) for n in prose.broad | tool.broad)
        prose_tagged = any(read.contains(n) for n in prose.explicit)
        tool_tagged = any(read.contains(n) for n in tool.explicit)
        same_file = any(path == item.call.path and read.contains(n) for path, n in tool.direct_file_offsets)
        self.risk.update({
            "numbered_reads_with_full_carries": 1,
            "reads_with_same_turn_followup": has_prose or has_call,
            "reads_with_any_matching_number": broad,
            "reads_with_explicit_matching_line_syntax": prose_tagged or tool_tagged,
            "reads_with_matching_prose_line_syntax": prose_tagged,
            "reads_with_matching_tool_line_syntax": tool_tagged,
            "reads_with_matching_explicit_same_file_reread_offset": same_file,
        })

    def price(self, encoding):
        cohorts = {}
        for label, stats in self.strata.items():
            sample = self.samples[label]
            opts = [[], []]
            for unit in sample:
                current = len(encoding.encode_ordinary(unit.read.text))
                for results, text in zip(opts, unit.read.transforms(), strict=True):
                    results.append(current - len(encoding.encode_ordinary(text)))
            cohorts[label] = dict(stats) | {"sample_size": len(sample), "options": {
                option: projection(stats, sample, values)
                for option, values in zip(("unpad", "range"), opts, strict=True)
            }}
            print(f"Tokenized size stratum {label} ({len(sample)} results).", file=sys.stderr, flush=True)
        aggregate = {}
        for option in ("unpad", "range"):
            rows = [c["options"][option] for c in cohorts.values()]
            tokens = sum(row["projected_token_turns"] for row in rows)
            se = math.sqrt(sum(row["projected_variance"] for row in rows))
            aggregate[option] = {
                "projected_token_turns": tokens,
                "token_turns_sample_count_expansion": sum(row["token_turns_sample_count_expansion"] for row in rows),
                "upper_carry_token_turns": sum(row["upper_carry_token_turns"] for row in rows),
                "sampling_interval_95": [tokens - 1.96 * se, tokens + 1.96 * se],
            }
        return cohorts, aggregate


def projection(stats, sample, values):
    n = len(sample)
    N = stats["population"]
    weighted_lines = [row.read.numbered * row.carries for row in sample]
    weighted_deltas = [delta * row.carries for row, delta in zip(sample, values, strict=True)]
    sample_total = sum(weighted_lines)
    ratio = sum(weighted_deltas) / sample_total if sample_total else 0.0
    projected = ratio * stats["numbered_line_carries"]
    mean_x = sample_total / n
    residuals = [delta - ratio * lines for lines, delta in zip(weighted_lines, weighted_deltas, strict=True)]
    sample_variance = sum(x * x for x in residuals) / (n - 1) if n > 1 else 0.0
    variance = ((stats["numbered_line_carries"] / mean_x) ** 2 * (1 - n / N) * sample_variance / n
                if mean_x else 0.0)
    upper_lines = sum(row.read.numbered * row.upper_carries for row in sample)
    upper_deltas = sum(delta * row.upper_carries for row, delta in zip(sample, values, strict=True))
    return {
        "per_result_tokens_saved": baseline.quantiles(values),
        "per_result_mean_tokens_saved": sum(values) / n,
        "rows_with_savings": sum(delta > 0 for delta in values),
        "rows_with_growth": sum(delta < 0 for delta in values),
        "sample_numbered_line_carries": sample_total,
        "projected_token_turns": projected,
        "projected_variance": variance,
        "token_turns_sample_count_expansion": sum(weighted_deltas) * N / n,
        "upper_carry_token_turns": upper_deltas / upper_lines * stats["upper_numbered_line_carries"] if upper_lines else 0,
    }


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
        audit.add(t)
        if index % 2500 == 0:
            print(f"Reconstructed {index} transcripts.", file=sys.stderr, flush=True)
    return recorded, audit


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("database", type=Path)
    parser.add_argument("--tokenizer-cache", required=True, type=Path)
    parser.add_argument("--json", required=True, type=Path, help="write a new aggregate-only file")
    args = parser.parse_args()
    if args.json.exists():
        parser.error("select a new JSON destination")
    encoding = offline_encoding(args.tokenizer_cache)
    with closing(baseline.open_read_only(args.database.expanduser())) as db:
        recorded, audit = analyze(db)
    strata, projected = audit.price(encoding)
    result = {
        "recorded_input": recorded,
        "tokenizer": encoding.name,
        "sample_seed": 921,
        "sampled_results": sum(len(s) for s in audit.samples.values()),
        "excluded": dict(audit.totals),
        "line_reference_proxies": dict(audit.risk),
        "strata": strata,
        "projected": projected,
    }
    with args.json.open("x", encoding="utf-8") as output:
        json.dump(result, output, indent=2, allow_nan=False)
        output.write("\n")


if __name__ == "__main__":
    main()
