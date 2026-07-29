#!/usr/bin/env python3
"""Render a bounded CPU-work report from one check profile artifact directory."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


def _records(root: Path, errors: list[dict]):
    for path in root.rglob("*.json"):
        if path.name == "summary.json":
            continue
        try:
            value = json.loads(path.read_text())
        except (OSError, ValueError) as error:
            errors.append({"source": str(path.relative_to(root)), "line": None, "error": str(error)})
            continue
        if "total_cpu_ms" in value or "cpu_ms" in value:
            yield path, value
    for path in root.glob("*.jsonl"):
        try:
            lines = path.read_text().splitlines()
        except OSError:
            continue
        for line_number, line in enumerate(lines, 1):
            try:
                value = json.loads(line)
            except ValueError as error:
                errors.append({
                    "source": str(path.relative_to(root)),
                    "line": line_number,
                    "error": str(error),
                })
                continue
            if "total_cpu_ms" in value or "cpu_user_us" in value:
                yield path, value


def _normalize(path: Path, value: dict) -> dict:
    cpu_ms = value.get("total_cpu_ms", value.get("cpu_ms"))
    if cpu_ms is None and "cpu_user_us" in value and "cpu_system_us" in value:
        cpu_ms = (value["cpu_user_us"] + value["cpu_system_us"]) / 1000.0
    identity = (
        value.get("identity")
        or value.get("full_name")
        or (
            f'{value["file"]}::{value["full_test_name"]}'
            if value.get("file") and value.get("full_test_name") else None
        )
        or value.get("full_test_name")
        or value.get("test_name")
        or path.stem
    )
    return {
        "identity": str(identity),
        "cpu_ms": float(cpu_ms) if cpu_ms is not None else None,
        "wall_ms": float(value.get("wall_ms", value.get("wall_time_ms", value.get("duration_ms", 0)))),
        "provenance": value.get("provenance", "unavailable"),
        "kind": value.get("kind", "step" if path.parent.name == "processes" else "test"),
        "source": str(path),
        "tree_closure": value.get("tree_closure"),
        "status": value.get("status"),
        "returncode": value.get("returncode"),
        "attempt": value.get("attempt"),
        "concurrent": value.get("concurrent"),
        "pid": value.get("pid"),
        "worker_id": value.get("worker_id"),
        "test_id": value.get("test_id"),
        "binary_id": value.get("binary_id"),
    }


def _reconciliation(rows: list[dict]) -> list[dict]:
    parent_names = {
        "rust": "rust-cargo-test.json",
        "vitest": "vitest-vitest.json",
        "python": "spec-shape-dev.py-unit-tests.json",
        "e2e": "e2e-e2e.json",
    }
    child_match = {
        "rust": lambda row: row["source"].startswith("rust-tests/"),
        "vitest": lambda row: row["source"].startswith("vitest-cpu-"),
        "python": lambda row: row["source"] == "python-test-cpu.jsonl",
        "e2e": lambda row: row["source"] == "e2e-scenario-cpu.jsonl",
    }
    result = []
    for name, parent_source in parent_names.items():
        parent = next((row for row in rows if row["source"] == f"processes/{parent_source}"), None)
        if parent is None or parent["cpu_ms"] is None:
            continue
        children = [
            row for row in rows
            if child_match[name](row) and row["cpu_ms"] is not None
        ]
        attributed = sum(row["cpu_ms"] for row in children)
        remainder = parent["cpu_ms"] - attributed
        result.append({
            "parent": name,
            "parent_cpu_ms": parent["cpu_ms"],
            "attributed_child_cpu_ms": attributed,
            "shared_unattributed_cpu_ms": max(0.0, remainder),
            "reconciliation_error_ms": min(0.0, remainder),
            "child_count": len(children),
            "children_may_overlap": any(
                row.get("concurrent") is True for row in children
            ),
        })
    return result


def render(profile_dir: Path, limit: int, run_id: str = "unknown") -> dict:
    profile_errors: list[dict] = []
    rows = [
        _normalize(path.relative_to(profile_dir), value)
        for path, value in _records(profile_dir, profile_errors)
    ]
    ranked_rows = sorted(
        (row for row in rows if row["cpu_ms"] is not None),
        key=lambda row: row["cpu_ms"], reverse=True,
    )
    unavailable_rows = [row for row in rows if row["cpu_ms"] is None]
    summary = {
        "schema_version": 1,
        "profile_dir": str(profile_dir),
        "profile_run_id": run_id,
        "record_count": len(rows),
        "top_cpu_records": ranked_rows[:limit],
        "unavailable_cpu_records": unavailable_rows,
        "reconciliation": _reconciliation(rows),
        "profile_errors": profile_errors,
        "traceql": {
            "command": f'{{ name = "dev.command" && span.check.profile_run_id = "{run_id}" }}',
            "steps": f'{{ name = "dev.check.step" && span.check.profile_run_id = "{run_id}" }}',
            "tests": f'{{ name = "dev.check.test" && span.check.profile_run_id = "{run_id}" }}',
        },
        "command": next((row for row in rows if row["kind"] == "command"), None),
        "run_metadata": next((
            value.get("metadata") for path, value in _records(profile_dir, [])
            if value.get("kind") == "command"
        ), None),
        "incomplete_tree_records": [
            row for row in rows if row.get("tree_closure") not in (None, "verified")
        ],
        "methodology": [
            "CPU milliseconds are additive work; wall time is secondary.",
            "Step process-tree totals are inclusive; do not add child test rows to them.",
            "windowed_process test rows can include shared worker or server activity.",
            "Sampled flamegraphs explain stacks but are not exact CPU accounting.",
        ],
    }
    (profile_dir / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
    lines = [
        "# Check CPU work profile", "",
        f"Records: {len(rows)}", "",
        "| CPU ms | Wall ms | Provenance | Outcome | Attempt | Concurrent | Closure | Kind | Identity |",
        "| ---: | ---: | --- | --- | --- | --- | --- | --- | --- |",
    ]
    for row in ranked_rows[:limit]:
        identity = row["identity"].replace("|", "\\|")
        closure = row.get("tree_closure") or "n/a"
        outcome = row.get("status") or (
            f'exit {row["returncode"]}' if row.get("returncode") is not None else "n/a"
        )
        attempt = row.get("attempt") or "n/a"
        concurrent = (
            "yes" if row.get("concurrent") is True
            else "no" if row.get("concurrent") is False
            else "n/a"
        )
        lines.append(
            f'| {row["cpu_ms"]:.1f} | {row["wall_ms"]:.1f} | '
            f'{row["provenance"]} | {outcome} | {attempt} | {concurrent} | '
            f'{closure} | {row["kind"]} | `{identity}` |'
        )
    if summary["run_metadata"]:
        lines += ["", "## Run metadata", "", "```json", json.dumps(summary["run_metadata"], indent=2, sort_keys=True), "```"]
    if profile_errors:
        lines += ["", "## Profile record errors", ""]
        for error in profile_errors:
            location = error["source"]
            if error["line"] is not None:
                location += f':{error["line"]}'
            lines.append(f'- `{location}`: {error["error"]}')
    if summary["reconciliation"]:
        lines += [
            "", "## Parent-child reconciliation", "",
            "| Parent | Inclusive CPU ms | Child CPU ms | Shared/unattributed ms | Error ms | Overlap risk |",
            "| --- | ---: | ---: | ---: | ---: | --- |",
        ]
        for item in summary["reconciliation"]:
            lines.append(
                f'| {item["parent"]} | {item["parent_cpu_ms"]:.1f} | '
                f'{item["attributed_child_cpu_ms"]:.1f} | '
                f'{item["shared_unattributed_cpu_ms"]:.1f} | '
                f'{item["reconciliation_error_ms"]:.1f} | '
                f'{"yes" if item["children_may_overlap"] else "no"} |'
            )
    if unavailable_rows:
        lines += ["", "## Unavailable CPU measurements", ""]
        lines += [f'- `{row["identity"]}` ({row["provenance"]})' for row in unavailable_rows]
    lines += ["", "## Interpretation", "", *[f"- {item}" for item in summary["methodology"]]]
    (profile_dir / "summary.md").write_text("\n".join(lines) + "\n")
    return summary


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("profile_dir", type=Path)
    parser.add_argument("--limit", type=int, default=30)
    parser.add_argument("--run-id", default="unknown")
    args = parser.parse_args()
    summary = render(args.profile_dir.resolve(), max(1, args.limit), args.run_id)
    print(args.profile_dir / "summary.md")
    print(f"ranked {summary['record_count']} CPU records")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
