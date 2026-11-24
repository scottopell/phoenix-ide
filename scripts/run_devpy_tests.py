#!/usr/bin/env python3
"""Run tests/devpy unittest modules concurrently within a bounded worker budget."""

from __future__ import annotations

import argparse
import concurrent.futures
import os
from pathlib import Path
import re
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
TESTS_DIR = ROOT / "tests" / "devpy"

# Run the slowest modules first so long tails overlap with fast modules.
_PRIORITY_MODULES = (
    "test_qualification_output",
    "test_systemd_deploy_helper",
    "test_seed",
    "test_simple_launchd_helper",
    "test_check_migration_ledger",
    "test_manual_sql_transactions",
    "test_check_profile_command",
    "test_compiler_cache",
    "test_systemd_deploy",
    "test_check_plan",
    "test_frozen_rc8_prepare",
    "test_frozen_rc5_release",
    "test_incident_stapler_controller",
)

_RAN_RE = re.compile(r"^Ran (\d+) tests? in ", re.MULTILINE)
_SKIP_RE = re.compile(r"skipped=(\d+)")


def discover_test_modules(tests_dir: Path = TESTS_DIR) -> list[str]:
    modules = sorted(
        path.stem
        for path in tests_dir.glob("test_*.py")
        if path.is_file()
    )
    rank = {name: idx for idx, name in enumerate(_PRIORITY_MODULES)}
    return sorted(modules, key=lambda name: (rank.get(name, len(rank)), name))


def _run_module(module: str, *, tests_dir: Path, verbose: bool) -> tuple[str, int, int, int, str]:
    cmd = [sys.executable, "-m", "unittest", module]
    if verbose:
        cmd.append("-v")
    env = os.environ.copy()
    existing_pythonpath = env.get("PYTHONPATH", "")
    extra = f"{ROOT}{os.pathsep}{tests_dir}"
    env["PYTHONPATH"] = f"{extra}{os.pathsep}{existing_pythonpath}" if existing_pythonpath else extra
    proc = subprocess.run(
        cmd,
        cwd=ROOT,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    output = proc.stdout or ""
    ran_match = _RAN_RE.search(output)
    ran = int(ran_match.group(1)) if ran_match else 0
    skip_match = _SKIP_RE.search(output)
    skipped = int(skip_match.group(1)) if skip_match else 0
    return module, proc.returncode, ran, skipped, output


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Run tests/devpy modules in parallel.")
    parser.add_argument("--workers", type=int, default=4, help="Maximum worker processes (>= 1)")
    parser.add_argument("-v", "--verbose", action="store_true", help="Verbose unittest output")
    parser.add_argument("--tests-dir", type=Path, default=TESTS_DIR, help="Test discovery directory")
    args = parser.parse_args(argv)

    workers = max(1, min(args.workers, os.cpu_count() or 1))
    modules = discover_test_modules(args.tests_dir)
    if not modules:
        print(f"No test_*.py modules found under {args.tests_dir}", file=sys.stderr)
        return 1

    started = time.monotonic()
    total_ran = 0
    total_skipped = 0
    failures: list[tuple[str, int, str]] = []

    with concurrent.futures.ThreadPoolExecutor(max_workers=min(workers, len(modules))) as pool:
        futures = [
            pool.submit(_run_module, mod, tests_dir=args.tests_dir, verbose=args.verbose)
            for mod in modules
        ]
        for future in concurrent.futures.as_completed(futures):
            mod, rc, ran, skipped, output = future.result()
            total_ran += ran
            total_skipped += skipped
            if rc != 0:
                failures.append((mod, rc, output))
            elif args.verbose and output:
                print(output, end="")

    elapsed = time.monotonic() - started
    for mod, rc, output in sorted(failures, key=lambda item: item[0]):
        print(f"\n=== FAILED: {mod} (exit {rc}) ===")
        if output:
            print(output, end="" if output.endswith("\n") else "\n")

    print(f"\n----------------------------------------------------------------------")
    print(f"Ran {total_ran} tests across {len(modules)} modules in {elapsed:.3f}s")
    print()
    if failures:
        suffix = f", skipped={total_skipped}" if total_skipped else ""
        print(f"FAILED (failed_modules={len(failures)}{suffix})")
        return 1
    if total_skipped:
        print(f"OK (skipped={total_skipped})")
    else:
        print("OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
