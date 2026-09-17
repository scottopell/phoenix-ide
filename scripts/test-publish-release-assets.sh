#!/bin/bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin" "$tmp/assets" "$tmp/state"

cat >"$tmp/bin/git" <<'PY'
#!/usr/bin/env python3
import os
import sys
from pathlib import Path
args = sys.argv[1:]
if args and args[0] == "fetch":
    raise SystemExit(0)
if args and args[0] == "rev-list":
    state = Path(os.environ["FAKE_STATE"])
    counter = state / "tag-check-count"
    count = int(counter.read_text()) + 1 if counter.exists() else 1
    counter.write_text(str(count))
    move_after = int(os.environ.get("FAKE_TAG_MOVE_AFTER", "0"))
    print("f" * 40 if move_after and count > move_after else os.environ["EXPECTED_COMMIT"])
    raise SystemExit(0)
raise SystemExit(2)
PY

cat >"$tmp/bin/gh" <<'PY'
#!/usr/bin/env python3
import hashlib
import json
import os
import sys
from pathlib import Path

state = Path(os.environ["FAKE_STATE"])
release_path = state / "release.json"
log_path = state / "gh.log"
args = sys.argv[1:]
with log_path.open("a", encoding="utf-8") as log:
    log.write(" ".join(args) + "\n")


def load():
    if not release_path.exists():
        return None
    return json.loads(release_path.read_text(encoding="utf-8"))


def save(release):
    release_path.write_text(json.dumps(release), encoding="utf-8")


def digest(asset):
    if asset["name"] == os.environ.get("FAKE_DIGEST_MISMATCH"):
        return "sha256:" + "f" * 64
    return "sha256:" + hashlib.sha256(Path(asset["source"]).read_bytes()).hexdigest()


def api_release(release):
    return {
        "id": release["id"],
        "tag_name": release["tag_name"],
        "draft": release["draft"],
        "upload_url": "repos/owner/repo/releases/42/assets{?name,label}",
        "assets": [
            {"id": asset["id"], "name": asset["name"], "digest": digest(asset)}
            for asset in release["assets"]
        ],
    }

if args[:2] == ["release", "create"]:
    tag = args[2]
    if load() is not None:
        raise SystemExit("release already exists")
    if "--draft" not in args:
        raise SystemExit("release must start as a draft")
    release = {"id": 42, "tag": tag, "tag_name": tag, "draft": True, "assets": [], "next_id": 100}
    save(release)
    raise SystemExit(0)

if args[:2] == ["release", "upload"]:
    release = load()
    if release is None or not release["draft"]:
        raise SystemExit("uploads require a draft")
    paths = [
        Path(value)
        for value in args[3:]
        if value != "--repo" and value != os.environ["FAKE_REPO"] and not value.startswith("-")
    ]
    fail_after = int(os.environ.get("FAKE_UPLOAD_FAIL_AFTER", "0"))
    for index, path in enumerate(paths, start=1):
        release["assets"].append(
            {"id": release["next_id"], "name": path.name, "source": str(path)}
        )
        release["next_id"] += 1
        save(release)
        if fail_after and index >= fail_after:
            raise SystemExit("simulated partial upload")
    raise SystemExit(0)

if args and args[0] == "api":
    method = "GET"
    filtered = []
    input_path = None
    slurp = "--slurp" in args
    index = 1
    while index < len(args):
        if args[index] == "--method":
            method = args[index + 1]
            index += 2
        elif args[index] == "--input":
            input_path = Path(args[index + 1])
            index += 2
        elif args[index] in {"-F", "-f", "-H", "--hostname"}:
            index += 2
        elif args[index] in {"--paginate", "--slurp"}:
            index += 1
        else:
            filtered.append(args[index])
            index += 1
    endpoint = filtered[0]
    release = load()
    if method == "GET" and "/releases/tags/" in endpoint:
        if release is None or release["draft"]:
            raise SystemExit(1)
        print(json.dumps(api_release(release)))
        raise SystemExit(0)
    if method == "GET" and endpoint.endswith("/releases?per_page=100"):
        page = [] if release is None or not release["draft"] else [api_release(release)]
        print(json.dumps([page] if slurp else page))
        raise SystemExit(0)
    if method == "GET" and endpoint.endswith("/releases/42"):
        if release is None:
            raise SystemExit(1)
        print(json.dumps(api_release(release)))
        raise SystemExit(0)
    if method == "POST" and "/releases/42/assets?name=" in endpoint:
        if release is None or not release["draft"] or input_path is None:
            raise SystemExit("asset upload requires a draft and input")
        fail_after = int(os.environ.get("FAKE_UPLOAD_FAIL_AFTER", "0"))
        upload_count = len(release["assets"]) + 1
        name = endpoint.split("?name=", 1)[1]
        release["assets"].append(
            {"id": release["next_id"], "name": name, "source": str(input_path)}
        )
        release["next_id"] += 1
        save(release)
        if fail_after and upload_count >= fail_after:
            raise SystemExit("simulated partial upload")
        print(json.dumps(api_release(release)["assets"][-1]))
        raise SystemExit(0)
    if method == "DELETE" and "/releases/assets/" in endpoint:
        asset_id = int(endpoint.rsplit("/", 1)[1])
        if release is None or not release["draft"]:
            raise SystemExit("asset deletion requires a draft")
        release["assets"] = [asset for asset in release["assets"] if asset["id"] != asset_id]
        save(release)
        raise SystemExit(0)
    if method == "PATCH" and "/releases/" in endpoint:
        if release is None or not release["draft"]:
            raise SystemExit("only a draft can be published")
        if os.environ.get("FAKE_PUBLISH_FAIL") == "1":
            raise SystemExit("simulated publish failure")
        release["draft"] = False
        save(release)
        raise SystemExit(0)
raise SystemExit(2)
PY
chmod +x "$tmp/bin/git" "$tmp/bin/gh"

export PATH="$tmp/bin:$PATH"
export FAKE_STATE="$tmp/state"
export FAKE_REPO=owner/repo
export EXPECTED_COMMIT=0123456789abcdef0123456789abcdef01234567
export GIT="$tmp/bin/git"

asset="$tmp/assets/Phoenix-macos-aarch64-apple-darwin-v1.2.3.zip"
checksums="$tmp/assets/SHA256SUMS"
printf 'desktop-bytes' >"$asset"
write_checksums() {
  python3 - "$asset" "$checksums" <<'PY'
import hashlib
import sys
from pathlib import Path
asset = Path(sys.argv[1])
Path(sys.argv[2]).write_text(
    f"{hashlib.sha256(asset.read_bytes()).hexdigest()}  {asset.name}\n",
    encoding="utf-8",
)
PY
}
write_checksums

publish() {
  bash "$root/scripts/publish-release-assets.sh" \
    "$FAKE_REPO" v1.2.3 "$EXPECTED_COMMIT" "$asset" "$checksums"
}

reset_state() {
  rm -rf "$FAKE_STATE"
  mkdir -p "$FAKE_STATE"
  unset FAKE_DIGEST_MISMATCH FAKE_PUBLISH_FAIL FAKE_TAG_MOVE_AFTER FAKE_UPLOAD_FAIL_AFTER
}

assert_draft() {
  python3 - "$FAKE_STATE/release.json" <<'PY'
import json
import sys
from pathlib import Path
release = json.loads(Path(sys.argv[1]).read_text())
assert release["draft"] is True
PY
}

assert_published_exact() {
  python3 - "$FAKE_STATE/release.json" <<'PY'
import json
import sys
from pathlib import Path
release = json.loads(Path(sys.argv[1]).read_text())
assert release["draft"] is False
assert sorted(asset["name"] for asset in release["assets"]) == [
    "Phoenix-macos-aarch64-apple-darwin-v1.2.3.zip",
    "SHA256SUMS",
]
PY
}

# Fresh publication remains private until the complete draft verifies.
reset_state
publish
assert_published_exact
grep -F -- '--draft' "$FAKE_STATE/gh.log" >/dev/null
grep -F -- '--hostname uploads.github.com --method POST' "$FAKE_STATE/gh.log" >/dev/null
if grep -F -- '--method DELETE repos/owner/repo/releases/42' "$FAKE_STATE/gh.log" >/dev/null; then
  echo "draft recovery must not delete the release" >&2
  exit 1
fi

# An interrupted draft upload remains private and is replaced exactly on retry.
reset_state
export FAKE_UPLOAD_FAIL_AFTER=1
if publish >/dev/null 2>&1; then
  echo "expected partial draft upload to fail" >&2
  exit 1
fi
assert_draft
unset FAKE_UPLOAD_FAIL_AFTER
publish
assert_published_exact
grep -F -- '--method DELETE repos/owner/repo/releases/assets/' "$FAKE_STATE/gh.log" >/dev/null
if grep -F -- '--method DELETE repos/owner/repo/releases/42' "$FAKE_STATE/gh.log" >/dev/null; then
  echo "partial draft recovery must not delete the release" >&2
  exit 1
fi

# Digest verification failure never publishes the draft.
reset_state
export FAKE_DIGEST_MISMATCH=SHA256SUMS
if publish >/dev/null 2>&1; then
  echo "expected draft digest mismatch to fail" >&2
  exit 1
fi
assert_draft
unset FAKE_DIGEST_MISMATCH

# Publication API failure leaves the complete release private.
reset_state
export FAKE_PUBLISH_FAIL=1
if publish >/dev/null 2>&1; then
  echo "expected draft publication failure" >&2
  exit 1
fi
assert_draft
unset FAKE_PUBLISH_FAIL

# A moved tag fails before the first release mutation.
reset_state
export FAKE_TAG_MOVE_AFTER=1
if publish >/dev/null 2>&1; then
  echo "expected moved tag to fail" >&2
  exit 1
fi
test ! -e "$FAKE_STATE/release.json"
unset FAKE_TAG_MOVE_AFTER

# A public release is immutable: an exact set is idempotent, any mismatch fails.
reset_state
publish
before=$(cat "$FAKE_STATE/release.json")
publish
test "$(cat "$FAKE_STATE/release.json")" = "$before"
python3 - "$FAKE_STATE/release.json" <<'PY'
import json
import sys
from pathlib import Path
path = Path(sys.argv[1])
release = json.loads(path.read_text())
release["assets"].pop()
path.write_text(json.dumps(release))
PY
if publish >/dev/null 2>&1; then
  echo "expected incomplete public release to fail closed" >&2
  exit 1
fi

# Local malformed checksum input fails before any release exists.
reset_state
printf 'malformed\n' >"$checksums"
if publish >/dev/null 2>&1; then
  echo "expected malformed SHA256SUMS to fail" >&2
  exit 1
fi
test ! -e "$FAKE_STATE/release.json"
write_checksums

echo "draft release publication regression checks passed"
