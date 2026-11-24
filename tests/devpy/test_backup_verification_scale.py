"""Tiny budget-boundary tests for the hosted backup qualification harness."""
from __future__ import annotations

import contextlib
import importlib.util
from pathlib import Path

import unittest


SCRIPT = Path(__file__).parents[1] / "integration" / "backup_verification_scale.py"
spec = importlib.util.spec_from_file_location("backup_verification_scale", SCRIPT)
scale = importlib.util.module_from_spec(spec)
assert spec and spec.loader
spec.loader.exec_module(scale)


class FakeClock:
    def __init__(self):
        self.now = 10.0

    def __call__(self):
        return self.now


class FakeHelper:
    BACKUP_VERIFICATION_TIMEOUT_SECS = 1200.0

    def __init__(self, clock, verification_seconds):
        self.clock = clock
        self.verification_seconds = verification_seconds
        self.deadline_seen = None

    @contextlib.contextmanager
    def sqlite_writer_guard(self, _database):
        self.clock.now += 4.0
        yield

    def create_verified_database_backup(self, _database, _directory, *, timeout, diagnostics):
        assert timeout == 1200.0
        self.clock.now += self.verification_seconds
        return Path("tiny-backup.sqlite")

    def _backup_proof(self, backup, deadline, diagnostics):
        self.deadline_seen = deadline
        return {"backup": backup}


class HostedBudgetTests(unittest.TestCase):
    def test_guard_acquisition_is_outside_verification_budget(self):
        clock = FakeClock()
        helper = FakeHelper(clock, 1198.0)
        result = scale.run_verified_backup(helper, Path("source.sqlite"), Path("unused"), {}, clock=clock)
        _backup, _proof, guard_elapsed, verification_elapsed, envelope_elapsed = result
        self.assertEqual(guard_elapsed, 4.0)
        self.assertEqual(verification_elapsed, 1198.0)
        self.assertEqual(envelope_elapsed, 1202.0)
        self.assertEqual(helper.deadline_seen, 1214.0)

    def test_verification_over_budget_is_rejected_without_fixture_generation(self):
        clock = FakeClock()
        helper = FakeHelper(clock, 1200.001)
        with self.assertRaisesRegex(RuntimeError, "exceeded 1200.0s"):
            scale.run_verified_backup(helper, Path("source.sqlite"), Path("unused"), {}, clock=clock)
