#!/usr/bin/env python3
"""Wait for successful main-branch source CI for an exact commit."""

import json
import re
import subprocess
import sys
import time
import urllib.parse

REPOSITORY = "scottopell/phoenix-ide"
WORKFLOW = ".github/workflows/ci.yml"
DEADLINE_SECONDS = 3600
POLL_SECONDS = 30
API_TIMEOUT_SECONDS = 20


def _api(endpoint, timeout):
    try:
        result = subprocess.run(
            ["gh", "api", endpoint], capture_output=True, text=True,
            check=True, timeout=timeout,
        )
        return json.loads(result.stdout)
    except (subprocess.SubprocessError, OSError, json.JSONDecodeError) as error:
        raise ValueError(f"GitHub Actions API request failed: {error}") from error


def _exact_run(run, sha):
    return (run.get("path") == WORKFLOW and run.get("head_sha") == sha
            and run.get("event") == "push" and run.get("head_branch") == "main")


def _positive_int(value, name):
    if type(value) is not int or value < 1:
        raise ValueError(f"malformed GitHub Actions run: invalid {name}")
    return value


def qualify(sha, *, api=None, monotonic=time.monotonic, sleep=time.sleep):
    if not isinstance(sha, str) or re.fullmatch(r"[0-9a-f]{40}", sha) is None:
        raise ValueError("commit SHA must be exactly 40 lowercase hexadecimal characters")
    api = api or _api
    query = urllib.parse.urlencode({"head_sha": sha, "event": "push", "per_page": 100})
    endpoint = f"repos/{REPOSITORY}/actions/workflows/ci.yml/runs?{query}"
    deadline = monotonic() + DEADLINE_SECONDS
    while True:
        remaining = deadline - monotonic()
        if remaining <= 0:
            raise ValueError("timed out waiting for exact source CI run")
        payload = api(endpoint, min(API_TIMEOUT_SECONDS, remaining))
        if monotonic() >= deadline:
            raise ValueError("timed out waiting for exact source CI run")
        if not isinstance(payload, dict) or not isinstance(payload.get("workflow_runs"), list):
            raise ValueError("malformed GitHub Actions workflow-runs response")
        runs = payload["workflow_runs"]
        if any(not isinstance(run, dict) for run in runs):
            raise ValueError("malformed GitHub Actions workflow run")
        matching = [run for run in runs if _exact_run(run, sha)]
        if matching:
            for run in matching:
                _positive_int(run.get("run_number"), "run_number")
                _positive_int(run.get("id"), "id")
            latest = max(matching, key=lambda run: run["run_number"])
            status = latest.get("status")
            if status == "completed":
                if latest.get("conclusion") != "success":
                    raise ValueError("latest exact source CI run did not succeed")
                attempt = _positive_int(latest.get("run_attempt"), "run_attempt")
                detail_remaining = deadline - monotonic()
                if detail_remaining <= 0:
                    raise ValueError("timed out waiting for exact source CI run")
                verified = api(f"repos/{REPOSITORY}/actions/runs/{latest['id']}",
                               min(API_TIMEOUT_SECONDS, detail_remaining))
                if monotonic() >= deadline:
                    raise ValueError("timed out waiting for exact source CI run")
                if not isinstance(verified, dict):
                    raise ValueError("malformed GitHub Actions run response")
                for key in ("id", "run_number", "head_sha", "path", "event", "head_branch", "run_attempt", "status", "conclusion"):
                    if verified.get(key) != latest.get(key):
                        raise ValueError(f"GitHub Actions run identity changed: {key}")
                if not _exact_run(verified, sha) or verified.get("status") != "completed" or verified.get("conclusion") != "success":
                    raise ValueError("exact source CI run is no longer successful")
                return latest["id"], attempt
            if status not in ("queued", "in_progress", "pending", "requested", "waiting"):
                raise ValueError("latest exact source CI run has an invalid status")
        remaining = deadline - monotonic()
        if remaining <= 0:
            raise ValueError("timed out waiting for exact source CI run")
        sleep(min(POLL_SECONDS, remaining))


def main(argv):
    if len(argv) != 1:
        raise ValueError("usage: release_source_ci.py <40-character-lowercase-commit-sha>")
    run_id, attempt = qualify(argv[0])
    print(f"qualification_ci_run={run_id}")
    print(f"qualification_ci_attempt={attempt}")


if __name__ == "__main__":
    try:
        main(sys.argv[1:])
    except ValueError as error:
        print(str(error), file=sys.stderr)
        raise SystemExit(1) from error

__all__ = ["qualify", "main"]
