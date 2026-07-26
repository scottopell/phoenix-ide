#!/usr/bin/env python3
"""Render a bounded CPU-work report from one check profile artifact directory."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


def _records(root: Path):
    for path in root.rglob("*.json"):
        if path.name == "summary.json":
            continue
        try:
            value = json.loads(path.read_text())
        except (OSError, ValueError):
            continue
        if "total_cpu_ms" in value or "cpu_ms" in value:
            yield path, value
    for path in root.glob("*.jsonl"):
        try:
            lines = path.read_text().splitlines()
        except OSError:
            continue
        for line in lines:
            try:
                value = json.loads(line)
            except ValueError:
                continue
            if "total_cpu_ms" in value or "cpu_user_us" in value:
                yield path, value


def _normalize(path: Path, value: dict) -> dict:
    cpu_ms = value.get("total_cpu_ms", value.get("cpu_ms"))
    if cpu_ms is None:
        cpu_ms = (value.get("cpu_user_us", 0) + value.get("cpu_system_us", 0)) / 1000.0
    identity = value.get("identity") or value.get("full_test_name") or value.get("test_name") or path.stem
    return {
        "identity": str(identity),
        "cpu_ms": float(cpu_ms),
        "wall_ms": float(value.get("wall_ms", value.get("wall_time_ms", value.get("duration_ms", 0)))),
        "provenance": value.get("provenance", "unavailable"),
        "kind": value.get("kind", "step" if path.parent.name == "processes" else "test"),
        "source": str(path),
    }


def render(profile_dir: Path, limit: int) -> dict:
    rows = sorted(
        (_normalize(path.relative_to(profile_dir), value) for path, value in _records(profile_dir)),
        key=lambda row: row["cpu_ms"], reverse=True,
    )
    summary = {
        "schema_version": 1,
        "profile_dir": str(profile_dir),
        "record_count": len(rows),
        "top_cpu_records": rows[:limit],
        "traceql": {
            "command": '{ name = "dev.command" && span.check.profile_work = true }',
            "steps": '{ name = "dev.check.step" && span.check.profile_run_id != nil }',
            "tests": '{ name = "dev.check.test" && span.check.profile_run_id != nil }',
        },
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
        "| CPU ms | Wall ms | Provenance | Kind | Identity |",
        "| ---: | ---: | --- | --- | --- |",
    ]
    for row in rows[:limit]:
        identity = row["identity"].replace("|", "\\|")
        lines.append(f'| {row["cpu_ms"]:.1f} | {row["wall_ms"]:.1f} | {row["provenance"]} | {row["kind"]} | `{identity}` |')
    lines += ["", "## Interpretation", "", *[f"- {item}" for item in summary["methodology"]]]
    (profile_dir / "summary.md").write_text("\n".join(lines) + "\n")
    return summary


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("profile_dir", type=Path)
    parser.add_argument("--limit", type=int, default=30)
    args = parser.parse_args()
    summary = render(args.profile_dir.resolve(), max(1, args.limit))
    print(args.profile_dir / "summary.md")
    print(f"ranked {summary['record_count']} CPU records")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
