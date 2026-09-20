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
if os.environ.get("FAKE_API_FAILURE") == "1" and args[:1] == ["api"]:
    raise SystemExit("simulated API failure")


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
        "prerelease": release.get("prerelease", False),
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
    fields = {}
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
        elif args[index] in {"-F", "-f"}:
            key, value = args[index + 1].split("=", 1)
            if args[index] == "-F" and value in {"true", "false"}:
                value = value == "true"
            fields[key] = value
            index += 2
        elif args[index] == "--hostname":
            raise SystemExit("upload requests must use an absolute uploads.github.com URL")
        elif args[index] == "-H":
            index += 2
        elif args[index] in {"--paginate", "--slurp"}:
            index += 1
        else:
            filtered.append(args[index])
            index += 1
    endpoint = filtered[0]
    release = load()
    if method == "POST" and endpoint.endswith("/releases"):
        if release is not None:
            raise SystemExit("release already exists")
        required = {
            "tag_name": os.environ["FAKE_TAG"],
            "name": os.environ["FAKE_TAG"],
            "target_commitish": os.environ["EXPECTED_COMMIT"],
            "draft": True,
            "prerelease": os.environ["FAKE_CHANNEL"] == "rc",
            "make_latest": "false" if os.environ["FAKE_CHANNEL"] == "rc" else "true",
            "generate_release_notes": True,
        }
        if fields != required:
            raise SystemExit(f"release creation metadata is not explicit and exact: {fields!r}")
        release = {
            "id": 42,
            "tag": fields["tag_name"],
            "tag_name": fields["tag_name"],
            "draft": True,
            "prerelease": fields["prerelease"],
            "make_latest": fields["make_latest"],
            "assets": [],
            "next_id": 100,
        }
        save(release)
        print(json.dumps(api_release(release)))
        raise SystemExit(0)
    if method == "GET" and endpoint.endswith("/releases/latest"):
        latest_tag = os.environ.get("FAKE_LATEST_TAG")
        if latest_tag is None:
            latest_tag = "v1.2.2" if os.environ["FAKE_CHANNEL"] == "rc" else os.environ["FAKE_TAG"]
        print(latest_tag if "--jq" in args else json.dumps({"tag_name": latest_tag}))
        raise SystemExit(0)
    if method == "GET" and "/releases/tags/" in endpoint:
        if release is None:
            raise SystemExit(1)
        print(json.dumps(api_release(release)))
        raise SystemExit(0)
    if method == "GET" and endpoint.endswith("/releases?per_page=100"):
        page = [] if release is None else [api_release(release)]
        latest_tag = os.environ.get("FAKE_LATEST_TAG")
        if latest_tag is not None and latest_tag != (release or {}).get("tag_name"):
            page.append({"tag_name": latest_tag, "draft": False, "prerelease": False, "assets": []})
        print(json.dumps([page] if slurp else page))
        raise SystemExit(0)
    if method == "GET" and endpoint.endswith("/releases/42"):
        if release is None:
            raise SystemExit(1)
        print(json.dumps(api_release(release)))
        raise SystemExit(0)
    if method == "POST" and "/releases/42/assets?name=" in endpoint:
        if not endpoint.startswith("https://uploads.github.com/"):
            raise SystemExit("asset upload must target uploads.github.com")
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
        required = {
            "draft": False,
            "prerelease": os.environ["FAKE_CHANNEL"] == "rc",
            "make_latest": "false" if os.environ["FAKE_CHANNEL"] == "rc" else "true",
        }
        if fields != required:
            raise SystemExit(f"publication metadata is not explicit and exact: {fields!r}")
        if os.environ.get("FAKE_PUBLISH_FAIL") == "1":
            raise SystemExit("simulated publish failure")
        release["draft"] = fields["draft"]
        release["prerelease"] = fields["prerelease"]
        release["make_latest"] = fields["make_latest"]
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
export FAKE_TAG=v1.2.3
export FAKE_CHANNEL=stable

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
    "$FAKE_REPO" "$FAKE_TAG" "$EXPECTED_COMMIT" "$FAKE_CHANNEL" "$asset" "$checksums"
}

reset_state() {
  rm -rf "$FAKE_STATE"
  mkdir -p "$FAKE_STATE"
  unset FAKE_DIGEST_MISMATCH FAKE_PUBLISH_FAIL FAKE_TAG_MOVE_AFTER FAKE_UPLOAD_FAIL_AFTER FAKE_API_FAILURE FAKE_LATEST_TAG
  export FAKE_TAG=v1.2.3
  export FAKE_CHANNEL=stable
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
  python3 - "$FAKE_STATE/release.json" "$FAKE_CHANNEL" <<'PY'
import json
import sys
from pathlib import Path
release = json.loads(Path(sys.argv[1]).read_text())
assert release["draft"] is False
channel = sys.argv[2]
assert release["prerelease"] is (channel == "rc")
assert release["make_latest"] == ("false" if channel == "rc" else "true")
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
grep -F -- '--method POST repos/owner/repo/releases' "$FAKE_STATE/gh.log" >/dev/null
grep -F -- '-F draft=true -F prerelease=false -f make_latest=true' "$FAKE_STATE/gh.log" >/dev/null
grep -F -- '-F draft=false -F prerelease=false -f make_latest=true' "$FAKE_STATE/gh.log" >/dev/null
grep -F -- '--method POST https://uploads.github.com/repos/owner/repo/releases/42/assets?name=' "$FAKE_STATE/gh.log" >/dev/null
if grep -F -- '--method DELETE repos/owner/repo/releases/42' "$FAKE_STATE/gh.log" >/dev/null; then
  echo "draft recovery must not delete the release" >&2
  exit 1
fi

# RC publication is explicitly a prerelease and never becomes latest, including an idempotent retry.
reset_state
export FAKE_TAG=v1.2.3-rc.1
export FAKE_CHANNEL=rc
publish
assert_published_exact
before=$(cat "$FAKE_STATE/release.json")
publish
test "$(cat "$FAKE_STATE/release.json")" = "$before"
grep -F -- '-F draft=true -F prerelease=true -f make_latest=false' "$FAKE_STATE/gh.log" >/dev/null
grep -F -- '-F draft=false -F prerelease=true -f make_latest=false' "$FAKE_STATE/gh.log" >/dev/null
export FAKE_LATEST_TAG="$FAKE_TAG"
if publish >/dev/null 2>&1; then
  echo "expected RC publication to reject becoming repository latest" >&2
  exit 1
fi
unset FAKE_LATEST_TAG

# Resuming an older stable draft must not replace a newer stable latest release.
reset_state
export FAKE_PUBLISH_FAIL=1
if publish >/dev/null 2>&1; then
  echo "expected interrupted stable publication" >&2
  exit 1
fi
unset FAKE_PUBLISH_FAIL
export FAKE_LATEST_TAG=v1.2.4
if publish >/dev/null 2>&1; then
  echo "expected older stable draft retry to refuse latest replacement" >&2
  exit 1
fi
python3 - "$FAKE_STATE/release.json" <<'PY'
import json
import sys
from pathlib import Path
release = json.loads(Path(sys.argv[1]).read_text())
assert release["draft"] is True
PY
unset FAKE_LATEST_TAG

# The supplied channel must be derived from the validated tag before any API mutation.
reset_state
export FAKE_TAG=v1.2.3-rc.1
export FAKE_CHANNEL=stable
if publish >/dev/null 2>&1; then
  echo "expected stable channel with an RC tag to fail" >&2
  exit 1
fi
test ! -e "$FAKE_STATE/gh.log"
reset_state
export FAKE_CHANNEL=rc
if publish >/dev/null 2>&1; then
  echo "expected RC channel with a stable tag to fail" >&2
  exit 1
fi
test ! -e "$FAKE_STATE/gh.log"

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
test "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["id"])' "$FAKE_STATE/release.json")" = 42
grep -F -- '--method DELETE repos/owner/repo/releases/assets/' "$FAKE_STATE/gh.log" >/dev/null
if grep -F -- '/releases/tags/' "$FAKE_STATE/gh.log" >/dev/null; then
  echo "draft recovery must classify visibility from the release inventory, not the ambiguous tag endpoint" >&2
  exit 1
fi
if grep -F -- '--method DELETE repos/owner/repo/releases/42' "$FAKE_STATE/gh.log" >/dev/null; then
  echo "partial draft recovery must not delete the release" >&2
  exit 1
fi

# An interrupted RC draft remains a prerelease, never latest, and is replaced exactly on retry.
reset_state
export FAKE_TAG=v1.2.3-rc.1
export FAKE_CHANNEL=rc
export FAKE_UPLOAD_FAIL_AFTER=1
if publish >/dev/null 2>&1; then
  echo "expected partial RC draft upload to fail" >&2
  exit 1
fi
assert_draft
python3 - "$FAKE_STATE/release.json" <<'PY'
import json
import sys
from pathlib import Path
release = json.loads(Path(sys.argv[1]).read_text())
assert release["prerelease"] is True
assert release["make_latest"] == "false"
PY
unset FAKE_UPLOAD_FAIL_AFTER
publish
assert_published_exact

# An existing draft with the wrong publication channel fails before asset mutation.
reset_state
export FAKE_UPLOAD_FAIL_AFTER=1
if publish >/dev/null 2>&1; then
  echo "expected partial draft upload to fail" >&2
  exit 1
fi
unset FAKE_UPLOAD_FAIL_AFTER
python3 - "$FAKE_STATE/release.json" <<'PY'
import json
import sys
from pathlib import Path
path = Path(sys.argv[1])
release = json.loads(path.read_text())
release["prerelease"] = True
path.write_text(json.dumps(release))
PY
before=$(cat "$FAKE_STATE/release.json")
log_lines=$(wc -l < "$FAKE_STATE/gh.log")
if publish >/dev/null 2>&1; then
  echo "expected stable publication to reject an RC draft" >&2
  exit 1
fi
test "$(cat "$FAKE_STATE/release.json")" = "$before"
tail -n "+$((log_lines + 1))" "$FAKE_STATE/gh.log" | grep -Ev -- '^api --paginate --slurp repos/owner/repo/releases\?per_page=100$' >"$tmp/unexpected-mutations" || true
test ! -s "$tmp/unexpected-mutations"

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

# Inventory API failure is not release absence and performs no mutation.
reset_state
export FAKE_API_FAILURE=1
if publish >/dev/null 2>&1; then
  echo "expected release inventory API failure" >&2
  exit 1
fi
test ! -e "$FAKE_STATE/release.json"
if grep -E 'release create|--method (POST|DELETE|PATCH)' "$FAKE_STATE/gh.log" >/dev/null; then
  echo "release inventory API failure must not mutate release state" >&2
  exit 1
fi
unset FAKE_API_FAILURE

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

# Existing public metadata with the wrong channel is immutable and fails closed.
reset_state
publish
python3 - "$FAKE_STATE/release.json" <<'PY'
import json
import sys
from pathlib import Path
path = Path(sys.argv[1])
release = json.loads(path.read_text())
release["prerelease"] = True
path.write_text(json.dumps(release))
PY
before=$(cat "$FAKE_STATE/release.json")
log_lines=$(wc -l < "$FAKE_STATE/gh.log")
if publish >/dev/null 2>&1; then
  echo "expected stable publication to reject an RC public release" >&2
  exit 1
fi
test "$(cat "$FAKE_STATE/release.json")" = "$before"
new_calls="$tmp/public-channel-calls"
tail -n "+$((log_lines + 1))" "$FAKE_STATE/gh.log" >"$new_calls"
if grep -E -- '--method (POST|DELETE|PATCH)' "$new_calls" >/dev/null; then
  echo "public channel mismatch must fail before mutation" >&2
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
