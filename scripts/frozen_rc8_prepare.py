#!/usr/bin/env python3
"""Single-use source gate for protected preparation of the frozen RC8 candidate."""

import hashlib
import json
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request

BASE = "86071b4cc2ab0f9efdb8a3debf03b8569baf26f9"
CANDIDATE = "d5d2a45651035f58f3231f0dc82fd9ef59b7cb3c"
TREE = "5ebc83729fc5630f8ffcd03477b0aaac1187982b"
VERSION = "0.13.2-rc.8"
HELPER = "scripts/launchd_deploy_helper.py"
HELPER_SHA256 = "936886046a098896cb9656326d705776a66ed9d472161fdafc27f865c40f832e"
REPOSITORY = "scottopell/phoenix-ide"
CI_WORKFLOW = ".github/workflows/ci.yml"


def git(repo, *args):
    return subprocess.run(["git", "-C", str(repo), *args], check=True,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE).stdout


def verify_candidate(repo=".", main_ref="origin/main"):
    """Verify immutable candidate objects; do not rely on the current checkout."""
    try:
        if git(repo, "rev-parse", f"{CANDIDATE}^{{commit}}").decode().strip() != CANDIDATE:
            raise ValueError("pinned RC8 candidate commit is unavailable")
        if git(repo, "show", "-s", "--format=%T", CANDIDATE).decode().strip() != TREE:
            raise ValueError("pinned RC8 candidate tree does not match")
        git(repo, "merge-base", "--is-ancestor", BASE, CANDIDATE)
        git(repo, "merge-base", "--is-ancestor", BASE, main_ref)
    except subprocess.CalledProcessError as error:
        raise ValueError("pinned RC8 candidate or RC7 base ancestry check failed") from error
    manifest = git(repo, "show", f"{CANDIDATE}:crates/phoenix-ide/Cargo.toml")
    lockfile = git(repo, "show", f"{CANDIDATE}:Cargo.lock")
    if manifest.count(VERSION.encode()) != 1:
        raise ValueError("pinned candidate manifest does not encode exactly RC8")
    lock_entry = b'name = "phoenix_ide"\nversion = "' + VERSION.encode() + b'"'
    if lockfile.count(lock_entry) != 1:
        raise ValueError("pinned candidate lockfile does not encode exactly RC8")
    digest = hashlib.sha256(git(repo, "show", f"{CANDIDATE}:{HELPER}")).hexdigest()
    if digest != HELPER_SHA256:
        raise ValueError("pinned RC8 release helper SHA-256 does not match")
    return {"commit": CANDIDATE, "tree": TREE, "helper_sha256": digest}


def latest_ci_run():
    query = urllib.parse.urlencode({"head_sha": CANDIDATE, "event": "workflow_dispatch", "per_page": 100})
    url = f"https://api.github.com/repos/{REPOSITORY}/actions/workflows/ci.yml/runs?{query}"
    request = urllib.request.Request(url, headers={
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
        "User-Agent": "phoenix-frozen-rc8-prepare",
    })
    try:
        with urllib.request.urlopen(request, timeout=15) as response:
            payload = json.load(response)
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError) as error:
        raise ValueError(f"unable to verify public GitHub CI evidence: {error}") from error
    runs = payload.get("workflow_runs")
    if not isinstance(runs, list):
        raise ValueError("GitHub CI response did not include workflow runs")
    matching = [run for run in runs if run.get("path") == CI_WORKFLOW and
                run.get("head_sha") == CANDIDATE and run.get("event") == "workflow_dispatch"]
    if not matching:
        raise ValueError("no workflow_dispatch CI run exists for the exact RC8 candidate")
    latest = max(matching, key=lambda run: int(run.get("run_number", 0)))
    if latest.get("status") != "completed" or latest.get("conclusion") != "success":
        raise ValueError("latest exact-candidate ci.yml workflow_dispatch run is not completed successfully")
    return latest


def gate(repo=".", tag_input=""):
    if tag_input:
        raise ValueError("prepare-frozen-rc8 rejects release-tag input")
    source = verify_candidate(repo)
    run = latest_ci_run()
    print(f"Frozen RC8 preparation gate passed: candidate={CANDIDATE} ci_run={run['id']}", file=sys.stderr)
    print("preparation=true")
    print("release=false")
    print("create_tag=false")
    print("tag=")
    print(f"version={VERSION}")
    print("channel=rc")
    print(f"commit={source['commit']}")
    print(f"source_tree={source['tree']}")
    print(f"helper_sha256={source['helper_sha256']}")
    print(f"qualification_ci_run={int(run['id'])}")
    print(f"qualification_ci_attempt={int(run['run_attempt'])}")


def provenance_json(controller_commit, run_id, run_attempt, ci_run, ci_attempt):
    if len(controller_commit) != 40 or any(c not in "0123456789abcdef" for c in controller_commit):
        raise ValueError("controller commit must be a full lowercase commit SHA")
    if controller_commit == CANDIDATE:
        raise ValueError("controller commit must be distinct from the candidate")
    for value in (run_id, run_attempt, ci_run, ci_attempt):
        if not str(value).isascii() or not str(value).isdigit() or int(value) < 1:
            raise ValueError("workflow run identity must be positive")
    return {
        "schema": 1,
        "operation": "prepare-frozen-rc8",
        "source_commit": CANDIDATE,
        "source_tree": TREE,
        "version": VERSION,
        "helper": HELPER,
        "helper_sha256": HELPER_SHA256,
        "controller_commit": controller_commit,
        "run_id": int(run_id),
        "run_attempt": int(run_attempt),
        "qualification_ci_run": int(ci_run),
        "qualification_ci_attempt": int(ci_attempt),
        "qualification_ci_url": f"https://github.com/{REPOSITORY}/actions/runs/{int(ci_run)}/attempts/{int(ci_attempt)}",
    }


def main(args):
    if args == ["gate"] or not args:
        gate()
        return
    if len(args) == 7 and args[0] == "provenance":
        _, path, controller, run_id, attempt, ci_run, ci_attempt = args
        actual_tree = git(".", "rev-parse", "HEAD^{tree}").decode().strip()
        with open(HELPER, "rb") as source_helper:
            helper = hashlib.sha256(source_helper.read()).hexdigest()
        if git(".", "rev-parse", "HEAD").decode().strip() != CANDIDATE or actual_tree != TREE or helper != HELPER_SHA256:
            raise ValueError("checked-out RC8 source or helper bytes differ from pinned identity")
        with open(path, "w", encoding="utf-8") as output:
            json.dump(provenance_json(controller, run_id, attempt, ci_run, ci_attempt), output, indent=2, sort_keys=True)
            output.write("\n")
        return
    raise SystemExit("frozen RC8 preparation accepts only gate or provenance; no source, version, tag, or run selectors")


if __name__ == "__main__":
    try:
        main(sys.argv[1:])
    except (ValueError, subprocess.CalledProcessError) as error:
        raise SystemExit(str(error)) from error

