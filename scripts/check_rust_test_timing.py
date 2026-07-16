#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Reject elapsed-time and unbounded-wait synchronization in Rust tests."""

from __future__ import annotations

import argparse
import io
import json
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
from collections import Counter
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path

RULES = """
id: phoenix-rust-test-item
language: Rust
severity: warning
rule:
  any:
    - kind: function_item
    - kind: mod_item
    - kind: attribute_item
---
id: phoenix-rust-test-sleep
language: Rust
severity: warning
rule:
  any:
    - pattern: tokio::time::sleep($DURATION)
    - pattern: std::thread::sleep($DURATION)
    - pattern: thread::sleep($DURATION)
---
id: phoenix-rust-test-bare-sleep
language: Rust
severity: warning
rule:
  pattern: sleep($DURATION)
---
id: phoenix-rust-test-event-wait
language: Rust
severity: warning
rule:
  any:
    - pattern: $RECEIVER.recv().await
    - pattern: $NOTIFY.notified().await
---
id: phoenix-rust-test-timeout
language: Rust
severity: warning
rule:
  any:
    - pattern: tokio::time::timeout($DURATION, $FUTURE)
    - pattern: timeout($DURATION, $FUTURE)
"""

TEST_ATTR = re.compile(r"#\s*\[\s*(?:tokio::)?test(?:\s*\([^]]*\))?\s*\]")
CFG_ATTR = re.compile(r"#\s*\[\s*cfg\s*\(([^]]*)\)\s*\]")
EXEMPTION = "test-timing-allow:"
SLEEP_IMPORT = re.compile(
    r"use\s+(?:tokio::time|std::thread)::(?:sleep|\{[^}]*\bsleep\b[^}]*\})\s*;"
)
TIMEOUT_IMPORT = re.compile(
    r"use\s+tokio::time::(?:timeout|\{[^}]*\btimeout\b[^}]*\})\s*;"
)


@dataclass(frozen=True)
class Finding:
    key: tuple[str, str, str]
    diagnostic: str


def _relative_path(filename: str) -> str:
    path = Path(filename)
    try:
        return str(path.resolve().relative_to(Path.cwd().resolve()))
    except ValueError:
        return str(path)


def _bounds(finding: dict) -> tuple[int, int]:
    offsets = finding["range"]["byteOffset"]
    return offsets["start"], offsets["end"]


def _is_trivia(source: bytes) -> bool:
    text = source.decode("utf-8", errors="ignore")
    text = re.sub(r"//[^\n]*(?:\n|$)", "", text)
    previous = None
    while previous != text:
        previous = text
        text = re.sub(r"/\*.*?\*/", "", text, flags=re.DOTALL)
    return not text.strip()


def _attached_attributes(source: bytes, start: int, attributes: list[dict]) -> str:
    cursor = start
    attached = []
    for attribute in sorted(attributes, key=lambda item: _bounds(item)[1], reverse=True):
        attr_start, attr_end = _bounds(attribute)
        if attr_end > cursor:
            continue
        if not _is_trivia(source[attr_end:cursor]):
            break
        attached.append(attribute["text"])
        cursor = attr_start
    return "\n".join(reversed(attached))


def _cfg_has_positive_test(expression: str) -> bool:
    expression = re.sub(r'r#*".*?"#*|"(?:\\.|[^"\\])*"', '""', expression)
    tokens = re.findall(r"[A-Za-z_][A-Za-z0-9_]*|[(),]", expression)

    def parse(index: int, negated: bool = False) -> tuple[bool, int]:
        if index >= len(tokens):
            return False, index
        token = tokens[index]
        if token == "not" and index + 1 < len(tokens) and tokens[index + 1] == "(":
            result, index = parse(index + 2, not negated)
            return result, index + (index < len(tokens) and tokens[index] == ")")
        if index + 1 < len(tokens) and tokens[index + 1] == "(":
            index += 2
            found = False
            while index < len(tokens) and tokens[index] != ")":
                child, index = parse(index, negated)
                found = found or child
                if index < len(tokens) and tokens[index] == ",":
                    index += 1
            return found, index + (index < len(tokens) and tokens[index] == ")")
        return token == "test" and not negated, index + 1

    return parse(0)[0]


def _attributes_enable_test(prefix: str) -> bool:
    return any(_cfg_has_positive_test(match.group(1)) for match in CFG_ATTR.finditer(prefix))


def _is_test_scope(
    filename: str,
    source: bytes,
    smell: dict,
    items: list[dict],
    attributes: list[dict],
) -> bool:
    path = Path(filename)
    if "tests" in path.parts or path.name in {"tests.rs", "testing.rs"}:
        return True
    point, _ = _bounds(smell)
    for item in items:
        start, end = _bounds(item)
        if not start <= point < end:
            continue
        prefix = _attached_attributes(source, start, attributes)
        if item["text"].lstrip().startswith("mod "):
            if _attributes_enable_test(prefix):
                return True
        elif TEST_ATTR.search(prefix) or _attributes_enable_test(prefix):
            return True
    return False


def _scope_key(smell: dict, items: list[dict]) -> str:
    point, _ = _bounds(smell)
    containers = [
        item for item in items
        if _bounds(item)[0] <= point < _bounds(item)[1]
    ]
    modules = []
    function = "<module>"
    for item in sorted(containers, key=lambda candidate: _bounds(candidate)[0]):
        text = item["text"].lstrip()
        match = re.match(r"mod\s+([A-Za-z_][A-Za-z0-9_]*)", text)
        if match:
            modules.append(match.group(1))
            continue
        match = re.search(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)", text.split("{", 1)[0])
        if match:
            function = match.group(1)
    return "::".join([*modules, function])


def _is_bounded(smell: dict, timeouts: list[dict]) -> bool:
    start, end = _bounds(smell)
    return any(low <= start and end <= high for low, high in map(_bounds, timeouts))


def _is_exempt(source: str, smell: dict) -> bool:
    line = smell["range"]["start"]["line"]
    lines = source.splitlines()
    if line == 0:
        return False
    marker = lines[line - 1].strip()
    return marker.startswith("//") and EXEMPTION in marker and marker.split(EXEMPTION, 1)[1].strip() != ""


def findings(
    paths: list[str],
    ast_grep: str = "ast-grep",
    source_root: Path | None = None,
) -> list[Finding]:
    command = [ast_grep, "scan", "--inline-rules", RULES, "--json=compact", *paths]
    result = subprocess.run(command, text=True, capture_output=True, check=False)
    if result.returncode not in (0, 1):
        raise RuntimeError(result.stderr.strip() or "ast-grep failed")
    matches = json.loads(result.stdout or "[]")
    grouped: dict[str, dict[str, list[dict]]] = defaultdict(lambda: defaultdict(list))
    for match in matches:
        grouped[match["file"]][match["ruleId"]].append(match)

    diagnostics: list[Finding] = []
    source_root = (source_root or Path.cwd()).resolve()
    for filename, kinds in grouped.items():
        source_text = Path(filename).read_text()
        source = source_text.encode()
        all_items = kinds["phoenix-rust-test-item"]
        attributes = [item for item in all_items if item["text"].lstrip().startswith("#")]
        items = [item for item in all_items if item not in attributes]
        timeouts = [
            timeout for timeout in kinds["phoenix-rust-test-timeout"]
            if not timeout["text"].lstrip().startswith("timeout(")
            or TIMEOUT_IMPORT.search(source_text)
        ]
        typed_smells = [
            *(('sleep', smell) for smell in kinds["phoenix-rust-test-sleep"]),
            *(('sleep', smell) for smell in kinds["phoenix-rust-test-bare-sleep"] if SLEEP_IMPORT.search(source_text)),
            *(('event', smell) for smell in kinds["phoenix-rust-test-event-wait"]),
        ]
        seen_ranges: set[tuple[int, int, str]] = set()
        for kind, smell in typed_smells:
            identity = (*_bounds(smell), kind)
            if identity in seen_ranges:
                continue
            seen_ranges.add(identity)
            try:
                relative = str(Path(filename).resolve().relative_to(source_root))
            except ValueError:
                relative = _relative_path(filename)
            if not _is_test_scope(relative, source, smell, items, attributes):
                continue
            is_event_wait = kind == "event"
            if is_event_wait and _is_bounded(smell, timeouts):
                continue
            if not is_event_wait and _is_exempt(source_text, smell):
                continue
            location = smell["range"]["start"]
            text = " ".join(smell["text"].split())
            diagnostics.append(Finding(
                key=(relative, _scope_key(smell, items), f"{kind}:{text}"),
                diagnostic=(
                    f"{relative}:{location['line'] + 1}:{location['column'] + 1}: "
                    f"test timing smell `{text}`; wait for observable evidence"
                    + (
                        f" or add `// {EXEMPTION} <why elapsed time is the behavior>` immediately above"
                        if not is_event_wait else " and bound event waits with tokio::time::timeout"
                    )
                ),
            ))
    return diagnostics


def _introduced(current: list[Finding], baseline: list[Finding]) -> list[Finding]:
    remaining = Counter(finding.key for finding in baseline)
    introduced = []
    for finding in current:
        if remaining[finding.key]:
            remaining[finding.key] -= 1
        else:
            introduced.append(finding)
    return introduced


def introduced_findings(base_sha: str, paths: list[str], ast_grep: str = "ast-grep") -> list[Finding]:
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        archive = subprocess.run(
            ["git", "archive", "--format=tar", base_sha, "--", *paths],
            capture_output=True,
            check=False,
        )
        if archive.returncode != 0:
            raise RuntimeError(archive.stderr.decode().strip() or f"git archive {base_sha} failed")
        with tarfile.open(fileobj=io.BytesIO(archive.stdout)) as bundle:
            bundle.extractall(root, filter="data")
        base_paths = [str(root / path) for path in paths if (root / path).exists()]
        baseline = findings(base_paths, ast_grep=ast_grep, source_root=root)

    current = findings(paths, ast_grep=ast_grep)
    return _introduced(current, baseline)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("paths", nargs="*", default=["crates/"])
    parser.add_argument("--base-sha", help="only report findings introduced since this resolved commit")
    parser.add_argument("--all", action="store_true", help="report the complete existing inventory")
    args = parser.parse_args()
    if not shutil.which("ast-grep"):
        print("check_rust_test_timing: ast-grep is required", file=sys.stderr)
        return 2
    try:
        diagnostics = (
            findings(args.paths)
            if args.all or not args.base_sha
            else introduced_findings(args.base_sha, args.paths)
        )
    except (OSError, RuntimeError, json.JSONDecodeError) as error:
        print(f"check_rust_test_timing: {error}", file=sys.stderr)
        return 2
    if diagnostics:
        print("\n".join(finding.diagnostic for finding in diagnostics), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
