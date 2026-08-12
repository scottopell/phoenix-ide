#!/bin/bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin" "$tmp/assets"

state_dir="$tmp/state"
mkdir -p "$state_dir"

cat >"$tmp/bin/gh" <<'EOF'
#!/usr/bin/env python3
import json
import os
import shutil
import sys
from pathlib import Path

state_dir = Path(os.environ['FAKE_GH_STATE'])
log_path = Path(os.environ['FAKE_GH_LOG'])
release_path = state_dir / 'release.json'
download_dir = state_dir / 'download'
next_id_path = state_dir / 'next_id'
delete_fail = set(filter(None, os.environ.get('FAKE_GH_DELETE_FAIL_NAMES', '').split(':')))
patch_fail = set(filter(None, os.environ.get('FAKE_GH_PATCH_FAIL_NAMES', '').split(':')))
upload_clobber = os.environ.get('FAKE_GH_UPLOAD_CLOBBER_FAIL', '') == '1'
upload_fail_after = int(os.environ.get('FAKE_GH_UPLOAD_FAIL_AFTER', '0'))
restore_skip = set(filter(None, os.environ.get('FAKE_GH_RESTORE_SKIP_NAMES', '').split(':')))


def log(args):
    with log_path.open('a', encoding='utf-8') as fh:
        fh.write(' '.join(args) + '\n')


def load_release():
    if not release_path.exists():
        return None
    return json.loads(release_path.read_text())


def save_release(release):
    release_path.write_text(json.dumps(release, sort_keys=True))


def next_id():
    value = int(next_id_path.read_text()) if next_id_path.exists() else 1000
    next_id_path.write_text(str(value + 1))
    return value


def ensure_release(tag):
    release = load_release()
    if release is None:
        raise SystemExit(1)
    if release['tag'] != tag:
        raise SystemExit(1)
    return release


def parse_repo_tag(args):
    repo = None
    tag = None
    i = 0
    while i < len(args):
        arg = args[i]
        if arg == '--repo':
            repo = args[i + 1]
            i += 2
        elif tag is None and not arg.startswith('-'):
            tag = arg
            i += 1
        else:
            i += 1
    return repo, tag


args = sys.argv[1:]
log(args)
if not args:
    raise SystemExit(2)

if args[:2] == ['release', 'view']:
    _, tag = parse_repo_tag(args[2:])
    ensure_release(tag)
    raise SystemExit(0)

if args[:2] == ['release', 'create']:
    tag = args[2]
    assets = [
        arg for arg in args[3:]
        if not arg.startswith('-')
        and arg != os.environ.get('FAKE_REPO')
        and arg != tag
    ]
    release = {'tag': tag, 'assets': []}
    for asset in assets:
        path = Path(asset)
        release['assets'].append({'id': next_id(), 'name': path.name, 'source': str(path)})
    save_release(release)
    raise SystemExit(0)

if args[:2] == ['release', 'download']:
    _, tag = parse_repo_tag(args[2:])
    release = ensure_release(tag)
    dest = Path(args[args.index('--dir') + 1])
    dest.mkdir(parents=True, exist_ok=True)
    for entry in release['assets']:
        src = download_dir / entry['name']
        if not src.exists():
            src.write_text(f"backup:{entry['name']}")
        shutil.copy2(src, dest / entry['name'])
    raise SystemExit(0)

if args[:2] == ['release', 'upload']:
    _, tag = parse_repo_tag(args[2:])
    release = ensure_release(tag)
    clobber = '--clobber' in args
    upload_args = [
        Path(arg)
        for arg in args[2:]
        if not arg.startswith('-') and arg not in {tag, os.environ.get('FAKE_REPO')}
    ]
    if clobber and upload_clobber:
        print('restore upload failed', file=sys.stderr)
        raise SystemExit(1)
    for index, path in enumerate(upload_args, start=1):
        if clobber and path.name in restore_skip:
            continue
        existing = [a for a in release['assets'] if a['name'] == path.name]
        if existing and not clobber:
            print(f'duplicate asset name: {path.name}', file=sys.stderr)
            raise SystemExit(1)
        if clobber:
            release['assets'] = [a for a in release['assets'] if a['name'] != path.name]
        release['assets'].append({'id': next_id(), 'name': path.name, 'source': str(path)})
        save_release(release)
        if upload_fail_after and index >= upload_fail_after:
            print('partial upload failed', file=sys.stderr)
            raise SystemExit(1)
    raise SystemExit(0)

if args[0] == 'api':
    method = 'GET'
    trimmed = []
    i = 1
    while i < len(args):
        if args[i] == '--method':
            method = args[i + 1]
            i += 2
        else:
            trimmed.append(args[i])
            i += 1
    fields = {}
    filtered = []
    i = 0
    while i < len(trimmed):
        if trimmed[i] == '-f':
            key, value = trimmed[i + 1].split('=', 1)
            fields[key] = value
            i += 2
        else:
            filtered.append(trimmed[i])
            i += 1
    endpoint = filtered[0]
    if method == 'GET' and endpoint.startswith('repos/') and '/releases/tags/' in endpoint:
        tag = endpoint.rsplit('/', 1)[1]
        release = ensure_release(tag)
        print(json.dumps({'assets': [{'id': a['id'], 'name': a['name']} for a in release['assets']]}))
        raise SystemExit(0)
    if method == 'DELETE' and '/releases/assets/' in endpoint:
        release = load_release()
        asset_id = int(endpoint.rsplit('/', 1)[1])
        for asset in list(release['assets']):
            if asset['id'] == asset_id:
                if asset['name'] in delete_fail:
                    print('delete failed', file=sys.stderr)
                    raise SystemExit(1)
                release['assets'].remove(asset)
                save_release(release)
                raise SystemExit(0)
        raise SystemExit(1)
    if method == 'PATCH' and '/releases/assets/' in endpoint:
        release = load_release()
        asset_id = int(endpoint.rsplit('/', 1)[1])
        for asset in release['assets']:
            if asset['id'] == asset_id:
                new_name = fields['name']
                if any(other['name'] == new_name and other['id'] != asset_id for other in release['assets']):
                    print('rename collision', file=sys.stderr)
                    raise SystemExit(1)
                if new_name in patch_fail:
                    print('patch failed', file=sys.stderr)
                    raise SystemExit(1)
                asset['name'] = new_name
                save_release(release)
                raise SystemExit(0)
        raise SystemExit(1)

raise SystemExit(2)
EOF

cat >"$tmp/bin/jq" <<'EOF'
#!/usr/bin/env python3
import json
import sys

args = sys.argv[1:]
name = None
i = 0
while i < len(args):
    if args[i] == '--arg' and args[i + 1] == 'name':
        name = args[i + 2]
        i += 3
    else:
        i += 1
payload = json.load(sys.stdin)
for asset in payload.get('assets', []):
    if asset.get('name') == name:
        print(asset.get('id'))
        raise SystemExit(0)
raise SystemExit(1)
EOF
chmod +x "$tmp/bin/gh" "$tmp/bin/jq"

export PATH="$tmp/bin:$PATH"
export FAKE_REPO="owner/repo"
export FAKE_GH_STATE="$state_dir"
export FAKE_GH_LOG="$tmp/gh.log"

write_release() {
  python3 - "$state_dir/release.json" "$state_dir/download" "$@" <<'PY'
import json
import sys
from pathlib import Path

release_path = Path(sys.argv[1])
download_dir = Path(sys.argv[2])
download_dir.mkdir(parents=True, exist_ok=True)
assets = []
for idx, name in enumerate(sys.argv[3:], start=1):
    assets.append({'id': idx, 'name': name})
    (download_dir / name).write_text(f'backup:{name}')
release_path.write_text(json.dumps({'tag': 'v1.2.3', 'assets': assets}))
Path(release_path.parent / 'next_id').write_text(str(len(assets) + 1))
PY
}

release_asset_names() {
  python3 - "$state_dir/release.json" <<'PY'
import json, sys
from pathlib import Path
release = json.loads(Path(sys.argv[1]).read_text())
for asset in sorted(a['name'] for a in release['assets']):
    print(asset)
PY
}

assert_release_names() {
  expected=$(printf '%s\n' "$@")
  actual=$(release_asset_names)
  if [[ "$actual" != "$expected" ]]; then
    echo "expected release assets:" >&2
    printf '%s\n' "$expected" >&2
    echo "actual release assets:" >&2
    printf '%s\n' "$actual" >&2
    exit 1
  fi
}

reset_env() {
  rm -rf "$state_dir"
  mkdir -p "$state_dir"
  : > "$tmp/gh.log"
  unset FAKE_GH_DELETE_FAIL_NAMES FAKE_GH_PATCH_FAIL_NAMES FAKE_GH_UPLOAD_CLOBBER_FAIL FAKE_GH_UPLOAD_FAIL_AFTER FAKE_GH_RESTORE_SKIP_NAMES
}

asset_one="$tmp/assets/Phoenix Desktop.zip"
asset_two="$tmp/assets/SHA 256 SUMS.txt"
printf 'desktop-bytes' > "$asset_one"
printf 'checksums' > "$asset_two"

# fresh create
reset_env
bash "$root/scripts/publish-release-assets.sh" "$FAKE_REPO" v1.2.3 "$asset_one" "$asset_two"
assert_release_names 'Phoenix Desktop.zip' 'SHA 256 SUMS.txt'
grep -F 'release create v1.2.3 --repo owner/repo --title v1.2.3 --generate-notes' "$tmp/gh.log" >/dev/null

# exact-tag staged replacement success
reset_env
write_release 'Phoenix Desktop.zip' 'SHA 256 SUMS.txt' 'old note.txt'
bash "$root/scripts/publish-release-assets.sh" "$FAKE_REPO" v1.2.3 "$asset_one" "$asset_two"
assert_release_names 'Phoenix Desktop.zip' 'SHA 256 SUMS.txt' 'old note.txt'
grep -E 'release upload v1.2.3 --repo owner/repo .*/Phoenix Desktop\.zip\.staged-' "$tmp/gh.log" >/dev/null
grep -E 'api repos/owner/repo/releases/tags/v1.2.3' "$tmp/gh.log" >/dev/null
grep -E 'api --method PATCH repos/owner/repo/releases/assets/[0-9]+ -f name=Phoenix Desktop\.zip' "$tmp/gh.log" >/dev/null
python3 - "$state_dir/release.json" <<'PY'
import json, sys
from pathlib import Path
assets = json.loads(Path(sys.argv[1]).read_text())['assets']
if any('.staged-' in asset['name'] for asset in assets):
    raise SystemExit('staged asset remained after successful replacement')
PY

# partial staging upload leaves previous asset set untouched and cleans debris
reset_env
write_release 'Phoenix Desktop.zip' 'SHA 256 SUMS.txt' 'old note.txt'
export FAKE_GH_UPLOAD_FAIL_AFTER=1
status=0
bash "$root/scripts/publish-release-assets.sh" "$FAKE_REPO" v1.2.3 "$asset_one" "$asset_two" >/dev/null 2>"$tmp/stage-fail.err" || status=$?
[[ "$status" -eq 1 ]]
assert_release_names 'Phoenix Desktop.zip' 'SHA 256 SUMS.txt' 'old note.txt'
if release_asset_names | grep -F '.staged-' >/dev/null; then
  echo 'staged debris remained after failed staging upload' >&2
  exit 1
fi
unset FAKE_GH_UPLOAD_FAIL_AFTER

# commit failure restores previous complete set and verifies
reset_env
write_release 'Phoenix Desktop.zip' 'SHA 256 SUMS.txt' 'old note.txt'
export FAKE_GH_DELETE_FAIL_NAMES='Phoenix Desktop.zip'
status=0
bash "$root/scripts/publish-release-assets.sh" "$FAKE_REPO" v1.2.3 "$asset_one" "$asset_two" >/dev/null 2>"$tmp/restore-success.err" || status=$?
[[ "$status" -eq 1 ]]
grep -F 'release asset replacement failed; restoring previous asset set' "$tmp/restore-success.err" >/dev/null
assert_release_names 'Phoenix Desktop.zip' 'SHA 256 SUMS.txt' 'old note.txt'
grep -E 'release upload v1.2.3 --repo owner/repo --clobber .*/old note\.txt' "$tmp/gh.log" >/dev/null
unset FAKE_GH_DELETE_FAIL_NAMES

# partial restore is detected by post-restore membership verification
reset_env
write_release 'Phoenix Desktop.zip' 'SHA 256 SUMS.txt' 'old note.txt'
export FAKE_GH_PATCH_FAIL_NAMES='SHA 256 SUMS.txt'
export FAKE_GH_RESTORE_SKIP_NAMES='SHA 256 SUMS.txt'
set +e
bash "$root/scripts/publish-release-assets.sh" "$FAKE_REPO" v1.2.3 "$asset_one" "$asset_two" >/dev/null 2>"$tmp/restore-partial.err"
status=$?
set -e
[[ "$status" -eq 2 ]]
grep -F 'error: previous release asset restoration failed' "$tmp/restore-partial.err" >/dev/null
unset FAKE_GH_PATCH_FAIL_NAMES FAKE_GH_RESTORE_SKIP_NAMES

# restore failure returns distinct failure
reset_env
write_release 'Phoenix Desktop.zip' 'SHA 256 SUMS.txt' 'old note.txt'
export FAKE_GH_DELETE_FAIL_NAMES='Phoenix Desktop.zip'
export FAKE_GH_UPLOAD_CLOBBER_FAIL=1
set +e
bash "$root/scripts/publish-release-assets.sh" "$FAKE_REPO" v1.2.3 "$asset_one" "$asset_two" >/dev/null 2>"$tmp/restore-fail.err"
status=$?
set -e
[[ "$status" -eq 2 ]]
grep -F 'error: previous release asset restoration failed' "$tmp/restore-fail.err" >/dev/null

echo 'publish release asset regression checks passed'
