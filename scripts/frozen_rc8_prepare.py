#!/usr/bin/env python3
"""Fixed source gates for protected preparation of frozen RC candidates."""

import hashlib
import json
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass

APPLICATION_BASE = "d3b57573ccf8662d05125338aad2307bb03da598"
BASE = "86071b4cc2ab0f9efdb8a3debf03b8569baf26f9"
CANDIDATE = "d5d2a45651035f58f3231f0dc82fd9ef59b7cb3c"
TREE = "5ebc83729fc5630f8ffcd03477b0aaac1187982b"
VERSION = "0.13.2-rc.8"
HELPER = "scripts/launchd_deploy_helper.py"
HELPER_SHA256 = "936886046a098896cb9656326d705776a66ed9d472161fdafc27f865c40f832e"
REPOSITORY = "scottopell/phoenix-ide"
CI_WORKFLOW = ".github/workflows/ci.yml"


@dataclass(frozen=True)
class FrozenCandidate:
    name: str
    base: str
    application_base: str
    commit: str
    tree: str
    version: str
    helper_sha256: str


def rc8_profile():
    return FrozenCandidate("RC8", BASE, APPLICATION_BASE, CANDIDATE, TREE, VERSION, HELPER_SHA256)


RC9 = FrozenCandidate(
    "RC9", "86071b4cc2ab0f9efdb8a3debf03b8569baf26f9",
    "d3b57573ccf8662d05125338aad2307bb03da598",
    "35ee3cacf590e755d62b88ed4f140b7ae4d7acc8",
    "57fa3c14e2830d06a629026086268c2e96279cd0",
    "0.13.2-rc.9",
    "ef126688b3b194bcb3ee7466011be66858a72f40a1f71b0d497825d9594fdb3e",
)

RC9_PATHS = frozenset({
    ".github/workflows/ci.yml", "Cargo.lock", "crates/phoenix-ide/Cargo.toml",
    "scripts/launchd_deploy_helper.py", "tests/devpy/test_backup_comparator.py",
    "tests/devpy/test_backup_verification_scale.py",
    "tests/devpy/test_simple_launchd_helper.py",
    "tests/integration/backup_verification_scale.py",
    "tests/integration/qualify_release_helper.py",
})


def git(repo, *args):
    return subprocess.run(["git", "-C", str(repo), *args], check=True,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE).stdout


def _verify_candidate(repo, main_ref, profile):
    """Verify immutable candidate objects; do not rely on the current checkout."""
    try:
        if git(repo, "rev-parse", f"{profile.commit}^{{commit}}").decode().strip() != profile.commit:
            raise ValueError(f"pinned {profile.name} candidate commit is unavailable")
        if git(repo, "show", "-s", "--format=%T", profile.commit).decode().strip() != profile.tree:
            raise ValueError(f"pinned {profile.name} candidate tree does not match")
        git(repo, "merge-base", "--is-ancestor", profile.base, profile.commit)
        if git(repo, "rev-parse", f"{profile.base}^").decode().strip() != profile.application_base:
            raise ValueError("frozen RC7 application base does not match")
        git(repo, "merge-base", "--is-ancestor", profile.application_base, main_ref)
        if profile is RC9:
            parents = git(repo, "rev-list", "--parents", "-n", "1", profile.commit).decode().split()
            if parents != [profile.commit, profile.base]:
                raise ValueError("pinned RC9 candidate must have the RC7 base as its sole direct parent")
            paths = set(git(repo, "diff-tree", "--no-commit-id", "--name-only", "-r", profile.commit).decode().splitlines())
            if paths != RC9_PATHS:
                raise ValueError("pinned RC9 candidate changed paths do not match the fixed set")
    except subprocess.CalledProcessError as error:
        raise ValueError(f"pinned {profile.name} candidate or RC7 base ancestry check failed") from error
    manifest = git(repo, "show", f"{profile.commit}:crates/phoenix-ide/Cargo.toml")
    lockfile = git(repo, "show", f"{profile.commit}:Cargo.lock")
    if manifest.count(profile.version.encode()) != 1:
        raise ValueError(f"pinned candidate manifest does not encode exactly {profile.name}")
    lock_entry = b'name = "phoenix_ide"\nversion = "' + profile.version.encode() + b'"'
    if lockfile.count(lock_entry) != 1:
        raise ValueError(f"pinned candidate lockfile does not encode exactly {profile.name}")
    if profile is RC9:
        old = b"0.13.2-rc.7"
        for path, candidate_bytes, marker in (
            ("crates/phoenix-ide/Cargo.toml", manifest, b'name = "phoenix_ide"\nversion = "' + old + b'"'),
            ("Cargo.lock", lockfile, b'name = "phoenix_ide"\nversion = "' + old + b'"'),
        ):
            base_bytes = git(repo, "show", f"{profile.base}:{path}")
            if base_bytes.count(marker) != 1 or candidate_bytes != base_bytes.replace(marker, marker.replace(old, profile.version.encode())):
                raise ValueError(f"pinned RC9 {path} differs from RC7 beyond its version replacement")
    digest = hashlib.sha256(git(repo, "show", f"{profile.commit}:{HELPER}")).hexdigest()
    if digest != profile.helper_sha256:
        raise ValueError(f"pinned {profile.name} release helper SHA-256 does not match")
    return {"commit": profile.commit, "tree": profile.tree, "helper_sha256": digest}


def verify_candidate(repo=".", main_ref="origin/main"):
    return _verify_candidate(repo, main_ref, rc8_profile())


def verify_rc9_candidate(repo=".", main_ref="origin/main"):
    return _verify_candidate(repo, main_ref, RC9)


def _latest_ci_run(profile):
    query = urllib.parse.urlencode({"head_sha": profile.commit, "event": "workflow_dispatch", "per_page": 100})
    url = f"https://api.github.com/repos/{REPOSITORY}/actions/workflows/ci.yml/runs?{query}"
    request = urllib.request.Request(url, headers={
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
        "User-Agent": f"phoenix-frozen-{profile.name.lower()}-prepare",
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
                run.get("head_sha") == profile.commit and run.get("event") == "workflow_dispatch"]
    if not matching:
        raise ValueError(f"no workflow_dispatch CI run exists for the exact {profile.name} candidate")
    latest = max(matching, key=lambda run: int(run.get("run_number", 0)))
    if latest.get("status") != "completed" or latest.get("conclusion") != "success":
        raise ValueError("latest exact-candidate ci.yml workflow_dispatch run is not completed successfully")
    return latest


def latest_ci_run():
    return _latest_ci_run(rc8_profile())


def latest_rc9_ci_run():
    return _latest_ci_run(RC9)


def gate(repo=".", tag_input=""):
    if tag_input:
        raise ValueError("prepare-frozen-rc8 rejects release-tag input")
    source = verify_candidate(repo)
    run = latest_ci_run()
    _emit_gate(rc8_profile(), source, run)


def gate_rc9(repo=".", tag_input=""):
    if tag_input:
        raise ValueError("prepare-frozen-rc9 rejects release-tag input")
    _emit_gate(RC9, verify_rc9_candidate(repo), latest_rc9_ci_run())


def _emit_gate(profile, source, run):
    print(f"Frozen {profile.name} preparation gate passed: candidate={profile.commit} ci_run={run['id']}", file=sys.stderr)
    print("preparation=true")
    print("release=false")
    print("create_tag=false")
    print("tag=")
    print(f"version={profile.version}")
    print("channel=rc")
    print(f"commit={source['commit']}")
    print(f"source_tree={source['tree']}")
    print(f"helper_sha256={source['helper_sha256']}")
    print(f"qualification_ci_run={int(run['id'])}")
    print(f"qualification_ci_attempt={int(run['run_attempt'])}")


def _provenance_json(profile, controller_commit, run_id, run_attempt, ci_run, ci_attempt):
    if len(controller_commit) != 40 or any(c not in "0123456789abcdef" for c in controller_commit):
        raise ValueError("controller commit must be a full lowercase commit SHA")
    if controller_commit == profile.commit:
        raise ValueError("controller commit must be distinct from the candidate")
    for value in (run_id, run_attempt, ci_run, ci_attempt):
        if not str(value).isascii() or not str(value).isdigit() or int(value) < 1:
            raise ValueError("workflow run identity must be positive")
    return {
        "schema": 1,
        "operation": f"prepare-frozen-{profile.name.lower()}",
        "source_commit": profile.commit,
        "source_tree": profile.tree,
        "version": profile.version,
        "helper": HELPER,
        "helper_sha256": profile.helper_sha256,
        "controller_commit": controller_commit,
        "run_id": int(run_id),
        "run_attempt": int(run_attempt),
        "qualification_ci_run": int(ci_run),
        "qualification_ci_attempt": int(ci_attempt),
        "qualification_ci_url": f"https://github.com/{REPOSITORY}/actions/runs/{int(ci_run)}/attempts/{int(ci_attempt)}",
    }


def provenance_json(controller_commit, run_id, run_attempt, ci_run, ci_attempt):
    return _provenance_json(rc8_profile(), controller_commit, run_id, run_attempt, ci_run, ci_attempt)


def rc9_provenance_json(controller_commit, run_id, run_attempt, ci_run, ci_attempt):
    return _provenance_json(RC9, controller_commit, run_id, run_attempt, ci_run, ci_attempt)


def _write_provenance(path, profile, controller, run_id, attempt, ci_run, ci_attempt, build):
    actual_tree = git(".", "rev-parse", "HEAD^{tree}").decode().strip()
    with open(HELPER, "rb") as source_helper:
        helper = hashlib.sha256(source_helper.read()).hexdigest()
    if git(".", "rev-parse", "HEAD").decode().strip() != profile.commit or actual_tree != profile.tree or helper != profile.helper_sha256:
        raise ValueError(f"checked-out {profile.name} source or helper bytes differ from pinned identity")
    with open(path, "w", encoding="utf-8") as output:
        json.dump(build(controller, run_id, attempt, ci_run, ci_attempt), output, indent=2, sort_keys=True)
        output.write("\n")


def main(args):
    if args == ["gate"] or not args:
        gate()
        return
    if args == ["gate-rc9"]:
        gate_rc9()
        return
    if len(args) == 7 and args[0] in ("provenance", "provenance-rc9"):
        command, path, controller, run_id, attempt, ci_run, ci_attempt = args
        profile, build = (rc8_profile(), provenance_json) if command == "provenance" else (RC9, rc9_provenance_json)
        _write_provenance(path, profile, controller, run_id, attempt, ci_run, ci_attempt, build)
        return
    raise SystemExit("frozen preparation accepts only gate, gate-rc9, provenance, or provenance-rc9; no source, version, tag, or run selectors")


if __name__ == "__main__":
    try:
        main(sys.argv[1:])
    except (ValueError, subprocess.CalledProcessError) as error:
        raise SystemExit(str(error)) from error

