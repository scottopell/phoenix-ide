#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.13"
# dependencies = []
# ///
"""Reject elapsed-time and unbounded-wait synchronization in Rust tests."""

from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

RULES = """
id: phoenix-rust-test-item
language: Rust
severity: warning
rule:
  any:
    - kind: function_item
    - kind: mod_item
---
id: phoenix-rust-test-timing-smell
language: Rust
severity: warning
rule:
  any:
    - pattern: tokio::time::sleep($DURATION)
    - pattern: std::thread::sleep($DURATION)
    - pattern: thread::sleep($DURATION)
    - pattern: $RECEIVER.recv().await
    - pattern: $NOTIFY.notified().await
---
id: phoenix-rust-test-timeout
language: Rust
severity: warning
rule:
  pattern: tokio::time::timeout($DURATION, $FUTURE)
"""

TEST_ATTR = re.compile(r"#\s*\[\s*(?:tokio::)?test(?:\s*\([^]]*\))?\s*\]")
CFG_TEST_ATTR = re.compile(r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]")
EXEMPTION = "test-timing-allow:"


def _relative_path(filename: str) -> str:
    path = Path(filename)
    try:
        return str(path.resolve().relative_to(Path.cwd().resolve()))
    except ValueError:
        return str(path)


def _bounds(finding: dict) -> tuple[int, int]:
    offsets = finding["range"]["byteOffset"]
    return offsets["start"], offsets["end"]


def _attribute_prefix(source: bytes, start: int) -> str:
    return source[max(0, start - 500) : start].decode("utf-8", errors="ignore")


def _attached_attributes(prefix: str) -> str:
    boundary = max(prefix.rfind("}"), prefix.rfind(";"))
    return prefix[boundary + 1 :]


def _is_test_scope(source: bytes, smell: dict, items: list[dict]) -> bool:
    point, _ = _bounds(smell)
    for item in items:
        start, end = _bounds(item)
        if not start <= point < end:
            continue
        prefix = _attached_attributes(_attribute_prefix(source, start))
        if item["text"].lstrip().startswith("mod "):
            if CFG_TEST_ATTR.search(prefix):
                return True
        elif TEST_ATTR.search(prefix):
            return True
    return False


def _is_bounded(smell: dict, timeouts: list[dict]) -> bool:
    start, end = _bounds(smell)
    return any(low <= start and end <= high for low, high in map(_bounds, timeouts))


def _changed_lines(base_sha: str, paths: list[str]) -> dict[str, set[int]]:
    result = subprocess.run(
        ["git", "diff", "--unified=0", base_sha, "--", *paths],
        text=True,
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(result.stderr.strip() or f"git diff against {base_sha} failed")
    changed: dict[str, set[int]] = defaultdict(set)
    filename: str | None = None
    for line in result.stdout.splitlines():
        if line.startswith("+++ b/"):
            filename = line[6:]
            continue
        if filename is None or not line.startswith("@@"):
            continue
        match = re.search(r"\+(\d+)(?:,(\d+))?", line)
        if match:
            start = int(match.group(1))
            count = int(match.group(2) or "1")
            changed[filename].update(range(start, start + count))
    for path in paths:
        candidate = Path(path)
        if candidate.is_file():
            relative = str(candidate).removeprefix("./")
            tracked = subprocess.run(
                ["git", "ls-files", "--error-unmatch", relative],
                capture_output=True,
                check=False,
            )
            if tracked.returncode != 0:
                changed[relative].update(range(1, candidate.read_text().count("\n") + 2))
    untracked = subprocess.run(
        ["git", "ls-files", "--others", "--exclude-standard", "--", *paths],
        text=True,
        capture_output=True,
        check=False,
    )
    if untracked.returncode != 0:
        raise RuntimeError(untracked.stderr.strip() or "cannot list untracked files")
    for filename in untracked.stdout.splitlines():
        candidate = Path(filename)
        if candidate.suffix == ".rs":
            changed[filename].update(range(1, candidate.read_text().count("\n") + 2))
    return changed


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
    changed_lines: dict[str, set[int]] | None = None,
) -> list[str]:
    command = [ast_grep, "scan", "--inline-rules", RULES, "--json=compact", *paths]
    result = subprocess.run(command, text=True, capture_output=True, check=False)
    if result.returncode not in (0, 1):
        raise RuntimeError(result.stderr.strip() or "ast-grep failed")
    matches = json.loads(result.stdout or "[]")
    grouped: dict[str, dict[str, list[dict]]] = defaultdict(lambda: defaultdict(list))
    for match in matches:
        grouped[match["file"]][match["ruleId"]].append(match)

    diagnostics = []
    for filename, kinds in grouped.items():
        source_text = Path(filename).read_text()
        source = source_text.encode()
        items = kinds["phoenix-rust-test-item"]
        timeouts = kinds["phoenix-rust-test-timeout"]
        for smell in kinds["phoenix-rust-test-timing-smell"]:
            start_line = smell["range"]["start"]["line"] + 1
            end_line = smell["range"]["end"]["line"] + 1
            relative = _relative_path(filename)
            if changed_lines is not None and not changed_lines.get(relative, set()).intersection(
                range(start_line, end_line + 1)
            ):
                continue
            if not _is_test_scope(source, smell, items):
                continue
            if ".recv().await" in smell["text"] or ".notified().await" in smell["text"]:
                if _is_bounded(smell, timeouts):
                    continue
            if _is_exempt(source_text, smell):
                continue
            location = smell["range"]["start"]
            diagnostics.append(
                f"{filename}:{location['line'] + 1}:{location['column'] + 1}: "
                f"test timing smell `{smell['text']}`; wait for observable evidence or add "
                f"`// {EXEMPTION} <why elapsed time is the behavior>` immediately above"
            )
    return diagnostics


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
        changed = None if args.all or not args.base_sha else _changed_lines(args.base_sha, args.paths)
        diagnostics = findings(args.paths, changed_lines=changed)
    except (OSError, RuntimeError, json.JSONDecodeError) as error:
        print(f"check_rust_test_timing: {error}", file=sys.stderr)
        return 2
    if diagnostics:
        print("\n".join(diagnostics), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
