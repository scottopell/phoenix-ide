#!/usr/bin/env python3
"""Single-use admission gate for publication of the fixed RC8 candidate."""

import importlib.util
import json
import subprocess
import sys
import urllib.error
import urllib.request
from pathlib import Path

PREP_PATH = Path(__file__).with_name("frozen_rc8_prepare.py")
_SPEC = importlib.util.spec_from_file_location("frozen_rc8_prepare_for_release", PREP_PATH)
_PREP = importlib.util.module_from_spec(_SPEC)
sys.modules[_SPEC.name] = _PREP
_SPEC.loader.exec_module(_PREP)

CANDIDATE = _PREP.CANDIDATE
VERSION = _PREP.VERSION
TAG = "v" + VERSION
REPOSITORY = _PREP.REPOSITORY
PREPARATION_RUN = 44975553897
PREPARATION_CONTROLLER = "b2af8a67d592a415cdf16e13eaf72c1b648ff072"


def api_json(url):
    request = urllib.request.Request(url, headers={
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
        "User-Agent": "phoenix-frozen-rc8-release",
    })
    try:
        with urllib.request.urlopen(request, timeout=15) as response:
            return json.load(response)
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError) as error:
        raise ValueError(f"unable to verify protected RC8 qualification evidence: {error}") from error


def verify_preparation_qualification():
    url = f"https://api.github.com/repos/{REPOSITORY}/actions/runs/{PREPARATION_RUN}"
    run = api_json(url)
    expected = {
        "id": PREPARATION_RUN,
        "path": ".github/workflows/release.yml",
        "event": "workflow_dispatch",
        "head_sha": PREPARATION_CONTROLLER,
        "head_branch": "main",
        "run_attempt": 1,
        "status": "completed",
        "conclusion": "success",
    }
    mismatches = [name for name, value in expected.items() if run.get(name) != value]
    if mismatches:
        raise ValueError("pinned protected RC8 preparation run is not completed successful exact-controller evidence: " +
                         ", ".join(mismatches))

    artifacts = api_json(url + "/artifacts?per_page=100").get("artifacts")
    if not isinstance(artifacts, list):
        raise ValueError("pinned preparation run did not expose its artifacts")
    names = {item.get("name") for item in artifacts if not item.get("expired")}
    required = {
        f"desktop-preparation-aarch64-apple-darwin-{CANDIDATE}",
        f"desktop-preparation-x86_64-apple-darwin-{CANDIDATE}",
        f"headless-acceptance-aarch64-apple-darwin-{CANDIDATE}-{PREPARATION_RUN}-1-success",
        f"headless-acceptance-x86_64-apple-darwin-{CANDIDATE}-{PREPARATION_RUN}-1-success",
    }
    if not required <= names:
        raise ValueError("pinned successful RC8 preparation run is missing expected native artifacts")
    return run


def verify_tag_unused(repo):
    remote = subprocess.check_output(
        ["git", "-C", str(repo), "ls-remote", "--tags", "--refs", "origin", "refs/tags/v*"],
        stderr=subprocess.PIPE).decode().splitlines()
    tags = [line.split("\t", 1)[-1].removeprefix("refs/tags/") for line in remote]
    if TAG in tags:
        raise ValueError(f"release tag already exists; refusing replacement: {TAG}")
    subprocess.run([sys.executable, "scripts/release_version.py", "validate-new-from-tags", VERSION],
                   cwd=repo, input=("\n".join(tags) + "\n").encode(), check=True,
                   stdout=subprocess.PIPE, stderr=subprocess.PIPE)


def gate(repo=".", tag_input=""):
    if tag_input:
        raise ValueError("release-frozen-rc8 rejects release-tag input")
    _PREP.verify_candidate(repo)
    qualification = _PREP.latest_ci_run()
    preparation = verify_preparation_qualification()
    verify_tag_unused(repo)
    print(f"Frozen RC8 release gate passed: candidate={CANDIDATE} ci_run={qualification['id']} "
          f"preparation_run={preparation['id']} tag={TAG}", file=sys.stderr)
    print("preparation=false")
    print("release=true")
    print("create_tag=true")
    print(f"tag={TAG}")
    print(f"version={VERSION}")
    print("channel=rc")
    print(f"commit={CANDIDATE}")
    print(f"qualification_ci_run={int(qualification['id'])}")
    print(f"qualification_ci_attempt={int(qualification['run_attempt'])}")


if __name__ == "__main__":
    if sys.argv[1:] not in ([], ["gate"]):
        raise SystemExit("frozen rc8 release gate accepts no candidate, base, tag, run, or source inputs")
    try:
        gate()
    except (ValueError, subprocess.CalledProcessError) as error:
        raise SystemExit(str(error)) from error
