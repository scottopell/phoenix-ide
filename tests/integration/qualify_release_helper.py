#!/usr/bin/env python3
"""Qualify the published release helper source without activating a service."""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import io
import json
import platform
import re
import sqlite3
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HELPER_PATH = "scripts/launchd_deploy_helper.py"
TEST_PATH = ROOT / "tests/devpy/test_simple_launchd_helper.py"
COMPARATOR_TEST_PATH = ROOT / "tests/devpy/test_backup_comparator.py"


def git(*args: str) -> bytes:
    return subprocess.run(
        ["git", *args], cwd=ROOT, check=True, capture_output=True
    ).stdout


def import_from_path(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load Python module from {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


class ProofQualificationResult(unittest.TextTestResult):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.successful_test_ids = set()

    def addSuccess(self, test):
        super().addSuccess(test)
        self.successful_test_ids.add(test.id())


def case_ids(suite):
    for test in suite:
        if isinstance(test, unittest.TestSuite):
            yield from case_ids(test)
        else:
            yield test.id()


def run(args: argparse.Namespace) -> int:
    if not re.fullmatch(r"[0-9a-fA-F]{40}", args.commit):
        raise SystemExit("--commit must be a full 40-character SHA")
    commit = args.commit.lower()
    head = git("rev-parse", "HEAD").decode().strip()
    if head != commit:
        raise SystemExit(f"checked-out HEAD {head} does not match requested commit {commit}")

    # Importing dev.py is required to exercise the production release-source path.
    dev = import_from_path("phoenix_qualify_dev", ROOT / "dev.py")
    expected = git("show", f"{commit}:{HELPER_PATH}")
    test_count = 0
    result = "failure"
    helper_sha = ""
    protocol = None
    test_output = ""
    with tempfile.TemporaryDirectory(prefix="phoenix-release-helper-qualification-") as temp:
        destination = Path(temp) / "activate.py"
        fixture_binary = Path(temp) / "candidate-binary-not-used"
        identity = dev.RuntimeIdentity(version="qualification-fixture", git_sha=commit)
        candidate = dev.PreparedCandidate(
            binary=fixture_binary,
            source_kind=dev.ProdSourceKind.PUBLISHED_RELEASE,
            source_commit=commit,
            identity=identity,
        )
        if candidate.source_kind is not dev.ProdSourceKind.PUBLISHED_RELEASE:
            raise SystemExit("candidate fixture is not a published release")
        # This exercises the actual GitHub raw-source transport; no local-source fallback.
        dev._materialize_helper(candidate.source_commit, destination, candidate.source_kind.value)
        materialized = destination.read_bytes()
        if materialized != expected:
            raise SystemExit("GitHub-materialized helper differs from git show at candidate commit")
        helper_sha = hashlib.sha256(materialized).hexdigest()

        protocol_run = subprocess.run(
            [sys.executable, str(destination), "--protocol-version"],
            check=False, capture_output=True, text=True, timeout=15,
        )
        if protocol_run.returncode != 0 or protocol_run.stdout.strip() != "3":
            raise SystemExit(
                "materialized helper does not report protocol 3 "
                f"(exit={protocol_run.returncode}, stdout={protocol_run.stdout.strip()!r})"
            )
        protocol = int(protocol_run.stdout.strip())

        # The subprocess proof witnesses use small disposable SQLite databases.
        test_module = import_from_path("phoenix_qualified_launchd_helper_tests", TEST_PATH)
        actual_helper = import_from_path("simple_launchd_helper_test", destination)
        test_module.helper = actual_helper
        comparator_tests = import_from_path("phoenix_qualified_comparator_tests", COMPARATOR_TEST_PATH)
        comparator_tests.helper = actual_helper
        suite = unittest.TestSuite([
            unittest.defaultTestLoader.loadTestsFromModule(test_module),
            unittest.defaultTestLoader.loadTestsFromModule(comparator_tests),
        ])
        required = [
            "phoenix_qualified_launchd_helper_tests.SimpleLaunchdHelperTests."
            "test_candidate_reopens_durable_migration_proof_before_migrating_existing_source",
            "phoenix_qualified_launchd_helper_tests.SimpleLaunchdHelperTests."
            "test_candidate_proof_witness_rejects_missing_pre_migration_publication",
        ]
        missing = set(required) - set(case_ids(suite))
        if missing:
            raise SystemExit(f"required proof-publication tests missing: {sorted(missing)}")
        test_count = suite.countTestCases()
        stream = io.StringIO()
        test_result = unittest.TextTestRunner(
            stream=stream, verbosity=2, resultclass=ProofQualificationResult,
        ).run(suite)
        test_output = stream.getvalue()
        successful_required = sorted(set(required) & test_result.successful_test_ids)
        if test_result.wasSuccessful() and successful_required == sorted(required):
            result = "passed"

    receipt = {
        "commit": commit,
        "suite_sha256": hashlib.sha256(TEST_PATH.read_bytes()).hexdigest(),
        "comparator_suite_sha256": hashlib.sha256(COMPARATOR_TEST_PATH.read_bytes()).hexdigest(),
        "materializer_sha256": hashlib.sha256((ROOT / "dev.py").read_bytes()).hexdigest(),
        "qualification_harness_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "working_tree_porcelain": git("status", "--porcelain").decode().splitlines(),
        "required_proof_publication_test_ids": required,
        "successful_proof_publication_test_ids": successful_required,
        "helper_path": HELPER_PATH,
        "helper_sha256": helper_sha,
        "protocol": protocol,
        "python": platform.python_version(),
        "sqlite": sqlite3.sqlite_version,
        "platform": platform.platform(),
        "tests": test_count,
        "result": result,
    }
    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(test_output, end="")
    print(json.dumps(receipt, sort_keys=True))
    return 0 if result == "passed" else 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--commit", required=True, help="full candidate commit SHA")
    parser.add_argument("--output", required=True, help="path for aggregate JSON receipt")
    return run(parser.parse_args())


if __name__ == "__main__":
    raise SystemExit(main())
