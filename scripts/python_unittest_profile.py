#!/usr/bin/env python3
"""Run unittest discovery and emit per-test CPU window records when enabled."""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import sys
try:
    import resource
except ImportError:
    resource = None
import time
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
_HELPER_PATH = ROOT / "scripts" / "check_profile_command.py"
_SPEC = importlib.util.spec_from_file_location("check_profile_command", _HELPER_PATH)
assert _SPEC is not None and _SPEC.loader is not None
_PROFILE = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(_PROFILE)

SCHEMA_VERSION = 1
PROVENANCE = "windowed_process"


def _append_jsonl(path: Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as output:
        json.dump(value, output, sort_keys=True)
        output.write("\n")


def _profile_dir_from_env() -> Path | None:
    configured = os.environ.get("PHOENIX_CHECK_PROFILE_DIR", "").strip()
    if not configured:
        return None
    return Path(configured)


def _jsonl_path(profile_dir: Path) -> Path:
    return profile_dir / "python-test-cpu.jsonl"


def _test_identity(test: unittest.case.TestCase) -> str:
    return f"python_unittest:{test.id()}"


def _cpu_times() -> tuple[float, float]:
    if resource is not None:
        own = resource.getrusage(resource.RUSAGE_SELF)
        children = resource.getrusage(resource.RUSAGE_CHILDREN)
        return own.ru_utime + children.ru_utime, own.ru_stime + children.ru_stime
    times = os.times()
    return times.user + times.children_user, times.system + times.children_system


class _ProfilingTextTestResult(unittest.TextTestResult):
    def __init__(self, stream, descriptions, verbosity, *, profile_jsonl: Path | None):
        super().__init__(stream, descriptions, verbosity)
        self._profile_jsonl = profile_jsonl
        self._started_wall_ns: int | None = None
        self._started_monotonic_ns: int | None = None
        self._started_cpu: tuple[float, float] | None = None
        self._outcome = "unknown"

    def startTest(self, test):
        self._started_wall_ns = time.time_ns()
        self._started_monotonic_ns = time.monotonic_ns()
        self._outcome = "running"
        self._started_cpu = _cpu_times()
        super().startTest(test)

    def addSuccess(self, test):
        self._outcome = "passed"
        super().addSuccess(test)

    def addFailure(self, test, err):
        self._outcome = "failed"
        super().addFailure(test, err)

    def addError(self, test, err):
        self._outcome = "error"
        super().addError(test, err)

    def addSkip(self, test, reason):
        self._outcome = "skipped"
        super().addSkip(test, reason)

    def addExpectedFailure(self, test, err):
        self._outcome = "expected_failure"
        super().addExpectedFailure(test, err)

    def addSubTest(self, test, subtest, err):
        if err is not None:
            failure_exception = getattr(test, "failureException", AssertionError)
            self._outcome = "failed" if issubclass(err[0], failure_exception) else "error"
        super().addSubTest(test, subtest, err)

    def addUnexpectedSuccess(self, test):
        self._outcome = "unexpected_success"
        super().addUnexpectedSuccess(test)

    def stopTest(self, test):
        try:
            if self._profile_jsonl is not None and self._started_cpu is not None:
                finished_monotonic_ns = time.monotonic_ns()
                user_total, system_total = _cpu_times()
                user_cpu_ms = (user_total - self._started_cpu[0]) * 1000.0
                system_cpu_ms = (system_total - self._started_cpu[1]) * 1000.0
                record = _PROFILE._record(
                    identity=_test_identity(test),
                    started_wall_ns=self._started_wall_ns or time.time_ns(),
                    started_monotonic_ns=self._started_monotonic_ns or finished_monotonic_ns,
                    finished_monotonic_ns=finished_monotonic_ns,
                    user_cpu_ms=user_cpu_ms,
                    system_cpu_ms=system_cpu_ms,
                    extra={
                        "schema_version": SCHEMA_VERSION,
                        "provenance": PROVENANCE,
                        "kind": "python_unittest_testcase",
                        "status": self._outcome,
                    },
                )
                _append_jsonl(self._profile_jsonl, record)
        finally:
            self._started_wall_ns = None
            self._started_monotonic_ns = None
            self._started_cpu = None
            self._outcome = "unknown"
            super().stopTest(test)


class _ProfilingTextTestRunner(unittest.TextTestRunner):
    resultclass = _ProfilingTextTestResult

    def __init__(self, *args, profile_jsonl: Path | None, **kwargs):
        super().__init__(*args, **kwargs)
        self._profile_jsonl = profile_jsonl

    def _makeResult(self):
        return self.resultclass(
            self.stream,
            self.descriptions,
            self.verbosity,
            profile_jsonl=self._profile_jsonl,
        )


class ProfilingProgram(unittest.TestProgram):
    def __init__(self, *, profile_jsonl: Path | None, argv: list[str]):
        self._profile_jsonl = profile_jsonl
        super().__init__(module=None, argv=argv, exit=False)

    def runTests(self):
        if self.testRunner is None:
            self.testRunner = _ProfilingTextTestRunner(
                verbosity=self.verbosity,
                failfast=self.failfast,
                buffer=self.buffer,
                warnings=self.warnings,
                tb_locals=self.tb_locals,
                profile_jsonl=self._profile_jsonl,
            )
        elif isinstance(self.testRunner, type):
            if issubclass(self.testRunner, _ProfilingTextTestRunner):
                self.testRunner = self.testRunner(
                    verbosity=self.verbosity,
                    failfast=self.failfast,
                    buffer=self.buffer,
                    warnings=self.warnings,
                    tb_locals=self.tb_locals,
                    profile_jsonl=self._profile_jsonl,
                )
            else:
                self.testRunner = self.testRunner(
                    verbosity=self.verbosity,
                    failfast=self.failfast,
                    buffer=self.buffer,
                    warnings=self.warnings,
                    tb_locals=self.tb_locals,
                )
        self.result = self.testRunner.run(self.test)
        if self.exit:
            sys.exit(not self.result.wasSuccessful())


def main(argv: list[str] | None = None) -> int:
    args = list(sys.argv[1:] if argv is None else argv)
    profile_dir = _profile_dir_from_env()
    program = ProfilingProgram(
        profile_jsonl=_jsonl_path(profile_dir) if profile_dir is not None else None,
        argv=[sys.argv[0], *args],
    )
    return 0 if program.result.wasSuccessful() else 1


if __name__ == "__main__":
    raise SystemExit(main())
