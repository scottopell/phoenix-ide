#!/usr/bin/env python3
"""Single-use gate for the frozen 0.13.2-rc.5 release candidate."""

import json
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request

BASE = "3b107af7536faf237e4d85fe0705a6a524a94229"
CANDIDATE = "4da5f81c60d65d548254264003540bc7d462d2b0"
TAG = "v0.13.2-rc.5"
VERSION = "0.13.2-rc.5"
OLD_VERSION = b"0.13.2-rc.3"
NEW_VERSION = b"0.13.2-rc.5"
MANIFEST = "crates/phoenix-ide/Cargo.toml"
LOCKFILE = "Cargo.lock"
REPOSITORY = "scottopell/phoenix-ide"
CI_WORKFLOW = ".github/workflows/ci.yml"


def git(repo, *args, input=None):
    return subprocess.run(
        ["git", "-C", str(repo), *args], input=input, check=True,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    ).stdout


def tree(repo, commit):
    # Empty subtrees have no leaf entries; reject them rather than silently omit them.
    for record in git(repo, "ls-tree", "-r", "-t", "-z", "--full-tree", commit).split(b"\0"):
        if record and record.split(b"\t", 1)[0].split()[1:] == [b"tree", b"4b825dc642cb6eb9a060e54bf8d69288fbee4904"]:
            raise ValueError("empty subtree is outside the frozen release contract")
    entries = git(repo, "ls-tree", "-r", "-z", "--full-tree", commit).split(b"\0")
    result = {}
    for entry in entries:
        if not entry:
            continue
        metadata, path = entry.split(b"\t", 1)
        mode, kind, oid = metadata.split(b" ", 2)
        result[path.decode("utf-8", "surrogateescape")] = (mode, kind, oid)
    return result


def substitute_once(data, description):
    if data.count(OLD_VERSION) != 1:
        raise ValueError(f"expected exactly one rc.3 version in {description}")
    return data.replace(OLD_VERSION, NEW_VERSION, 1)


def expected_lockfile(data):
    old = b'name = "phoenix_ide"\nversion = "' + OLD_VERSION + b'"'
    new = b'name = "phoenix_ide"\nversion = "' + NEW_VERSION + b'"'
    if data.count(old) != 1:
        raise ValueError("expected exactly one phoenix_ide rc.3 package in Cargo.lock")
    return data.replace(old, new, 1)


def verify_candidate(repo):
    if git(repo, "rev-parse", f"{CANDIDATE}^{{commit}}").decode().strip() != CANDIDATE:
        raise ValueError("pinned candidate commit is unavailable")
    parents = git(repo, "show", "-s", "--format=%P", CANDIDATE).decode().strip().split()
    if parents != [BASE]:
        raise ValueError("candidate must have the frozen application base as its sole parent")

    base_tree = tree(repo, BASE)
    candidate_tree = tree(repo, CANDIDATE)
    if set(base_tree) != set(candidate_tree):
        raise ValueError("candidate tracked paths differ from frozen application base")

    expected = dict(base_tree)
    for path in (MANIFEST, LOCKFILE):
        if path not in base_tree or base_tree[path][1] != b"blob":
            raise ValueError(f"frozen base is missing expected file {path}")
        base_bytes = git(repo, "cat-file", "blob", BASE + ":" + path)
        if path == MANIFEST:
            updated = substitute_once(base_bytes, path)
        else:
            updated = expected_lockfile(base_bytes)
        blob = git(repo, "hash-object", "--stdin", input=updated).strip()
        expected[path] = (base_tree[path][0], b"blob", blob)

    if candidate_tree != expected:
        mismatches = sorted(path for path in expected if candidate_tree[path] != expected[path])
        raise ValueError("candidate differs from exact two-file/version-only tree: " + ", ".join(mismatches[:8]))
    candidate_manifest = git(repo, "cat-file", "blob", CANDIDATE + ":" + MANIFEST)
    if candidate_manifest.count(NEW_VERSION) != 1:
        raise ValueError("candidate manifest does not encode exactly one rc.5 version")
    return True


def workflow_dispatch_ci_run():
    """Require the newest matching manual ci.yml run to have completed successfully."""
    query = urllib.parse.urlencode({"head_sha": CANDIDATE, "event": "workflow_dispatch", "per_page": 100})
    url = f"https://api.github.com/repos/{REPOSITORY}/actions/workflows/ci.yml/runs?{query}"
    request = urllib.request.Request(url, headers={"Accept": "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28", "User-Agent": "phoenix-frozen-rc5-gate"})
    try:
        with urllib.request.urlopen(request, timeout=15) as response:
            payload = json.load(response)
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError) as error:
        raise ValueError(f"unable to verify public GitHub CI evidence: {error}") from error
    runs = payload.get("workflow_runs")
    if not isinstance(runs, list):
        raise ValueError("GitHub CI response did not include workflow runs")
    matching = [run for run in runs if
        run.get("path") == CI_WORKFLOW and
        run.get("head_sha") == CANDIDATE and
        run.get("event") == "workflow_dispatch"]
    if not matching:
        raise ValueError("no workflow_dispatch run for exact candidate in ci.yml")
    latest = max(matching, key=lambda run: int(run.get("run_number", 0)))
    if latest.get("status") != "completed" or latest.get("conclusion") != "success":
        raise ValueError("latest exact-candidate ci.yml workflow_dispatch run is not completed successfully")
    return latest


def verify_tag_unused(repo):
    tags = git(repo, "tag", "--list", "v*").decode().splitlines()
    if TAG in tags:
        raise ValueError(f"release tag already exists; refusing replacement: {TAG}")
    subprocess.run(["python3", "scripts/release_version.py", "validate-new-from-tags", VERSION],
                   cwd=repo, input=("\n".join(tags) + "\n").encode(), check=True,
                   stdout=subprocess.PIPE, stderr=subprocess.PIPE)


def gate(repo="."):
    verify_candidate(repo)
    run = workflow_dispatch_ci_run()
    verify_tag_unused(repo)
    print(f"Frozen release gate passed: candidate={CANDIDATE} ci_run={run['id']} tag={TAG}", file=sys.stderr)
    print("preparation=false")
    print("release=true")
    print("create_tag=true")
    print(f"tag={TAG}")
    print(f"version={VERSION}")
    print("channel=rc")
    print(f"commit={CANDIDATE}")


if __name__ == "__main__":
    if sys.argv[1:] not in ([], ["gate"]):
        raise SystemExit("frozen rc5 gate accepts no candidate, base, tag, or run inputs")
    try:
        gate()
    except (ValueError, subprocess.CalledProcessError) as error:
        raise SystemExit(str(error)) from error


__all__ = ["BASE", "CANDIDATE", "TAG", "VERSION", "tree", "verify_candidate", "workflow_dispatch_ci_run"]
