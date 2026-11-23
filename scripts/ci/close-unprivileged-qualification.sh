#!/bin/bash
# Usage: bash close-unprivileged-qualification.sh WORKSPACE TEST_BINARY LAUNCHER RUNNER_UID SELECTOR [OPTIONS...]
# Build as the runner first; tee evidence outside this wrapper. The workflow bounds test duration.
set -euo pipefail

fail() {
    printf 'close qualification: %s\n' "$1" >&2
    exit 1
}

[[ $# -ge 5 ]] || fail 'expected workspace, test binary, launcher, runner UID, and test selector/options'
[[ $1 == /* && $2 == /* && $3 == /* ]] || fail 'workspace and binary paths must be absolute'
[[ $4 =~ ^[1-9][0-9]*$ ]] || fail 'runner UID must be numeric and nonroot'
[[ $UID == "$4" && $EUID == "$4" ]] || fail 'caller must be the specified nonroot runner'

workspace=$1
source_runtime_binary=$2
source_launcher_binary=$3
runner_uid=$4
shift 4
expected_test='runtime::executor::creation_completion_lifecycle_tests::native_close_admission_tests::worktree_physical::registered_native_close_with_production_detector_retires_worktree_and_replays'
[[ $# == 4 && $1 == "$expected_test" && $2 == --exact && $3 == --nocapture && $4 == --test-threads=1 ]] \
    || fail 'identity wrapper accepts only the exact disposable Worktree production-detector journey'

for binary in "$source_runtime_binary" "$source_launcher_binary"; do
    [[ -f $binary && -r $binary && -x $binary && ! -u $binary && ! -g $binary ]] \
        || fail 'source test and launcher binaries must be readable executable files without setuid/setgid'
done

# Stream the already-built artifacts into a nobody-owned private directory. This
# changes no checkout permissions and does not invoke a build as the dropped user.
scratch=$(/usr/bin/sudo -n -u nobody -- /usr/bin/mktemp -d /tmp/phoenix-close-qualification.XXXXXXXXXX) \
    || fail 'could not create nobody-owned private scratch'
cleanup_staging() {
    /usr/bin/sudo -n -u nobody -- /bin/rm -rf -- "$scratch"
}
trap cleanup_staging EXIT
stage_binary() {
    local source=$1 destination=$2 expected actual
    expected=$(/usr/bin/sha256sum -- "$source" | /usr/bin/cut -d ' ' -f 1) \
        || fail 'could not checksum source binary'
    /bin/cat -- "$source" | /usr/bin/sudo -n -u nobody -- /usr/bin/tee -- "$destination" >/dev/null \
        || fail 'could not stream binary into nobody-owned scratch'
    /usr/bin/sudo -n -u nobody -- /bin/chmod 700 -- "$destination" \
        || fail 'could not make staged binary executable'
    actual=$(/usr/bin/sudo -n -u nobody -- /usr/bin/sha256sum -- "$destination" | /usr/bin/cut -d ' ' -f 1) \
        || fail 'could not checksum staged binary'
    [[ $expected == "$actual" ]] || fail 'staged binary checksum differs from source'
}
stage_binary "$source_runtime_binary" "$scratch/runtime-test"
stage_binary "$source_launcher_binary" "$scratch/bash-launcher"

exec /usr/bin/sudo -n -u nobody -- /usr/bin/env -i \
    PATH=/usr/bin:/bin LC_ALL=C USER=nobody LOGNAME=nobody \
    /bin/bash --noprofile --norc -s -- "$workspace" "$scratch/runtime-test" "$scratch/bash-launcher" "$runner_uid" "$scratch" "$@" <<'DROPPED'
set -euo pipefail
umask 077
workspace=$1
runtime_binary=$2
launcher_binary=$3
runner_uid=$4
scratch=$5
shift 5
preflight_cleanup() {
    status=$?
    trap - EXIT
    if (( status != 0 )); then
        rm -rf -- "$scratch"
    fi
    exit "$status"
}
trap preflight_cleanup EXIT
expected_test='runtime::executor::creation_completion_lifecycle_tests::native_close_admission_tests::worktree_physical::registered_native_close_with_production_detector_retires_worktree_and_replays'
[[ $# == 4 && $1 == "$expected_test" && $2 == --exact && $3 == --nocapture && $4 == --test-threads=1 ]] || {
    echo 'identity wrapper accepts only the exact disposable Worktree production-detector journey' >&2
    exit 2
}

# Bound preflight independently of the same-identity test deadline below.
/usr/bin/timeout --kill-after=1s 10s /bin/bash --noprofile --norc -s -- \
    "$scratch" "$runtime_binary" "$launcher_binary" "$runner_uid" <<'PREFLIGHT'
set -euo pipefail
fail() {
    printf 'close qualification preflight: %s\n' "$1" >&2
    exit 1
}
scratch=$1
runtime_binary=$2
launcher_binary=$3
runner_uid=$4
for tool in bash git sleep timeout tmux id readlink stat mktemp mkdir rm; do
    command -v "$tool" >/dev/null || fail 'required system tool is missing'
done
uid=$(id -u nobody)
gid=$(id -g nobody)
[[ $uid =~ ^[1-9][0-9]*$ && $gid =~ ^[1-9][0-9]*$ ]] || fail 'nobody must have a nonroot numeric UID and GID'
[[ $UID == "$uid" && $EUID == "$uid" && $uid != "$runner_uid" ]] || fail 'selected identity is not the distinct nobody account'
[[ $(id -un) == nobody && $(id -g) == "$gid" ]] || fail 'selected account or primary group does not match nobody'

uid_seen=0
gid_seen=0
groups_seen=0
caps_seen=0
while IFS=$' \t' read -r key value; do
    case "$key" in
        Uid:|Gid:)
            read -r real effective saved fs extra <<< "$value"
            expected=$uid
            [[ $key == Uid: ]] || expected=$gid
            [[ $real == "$expected" && $effective == "$expected" && $saved == "$expected" && $fs == "$expected" && -z $extra ]] \
                || fail 'kernel real/effective/saved/filesystem credentials differ'
            if [[ $key == Uid: ]]; then uid_seen=1; else gid_seen=1; fi
            printf '%s %s %s %s %s\n' "$key" "$real" "$effective" "$saved" "$fs"
            ;;
        Groups:)
            read -r -a groups <<< "$value"
            for group in "${groups[@]}"; do
                [[ $group == "$gid" ]] || fail 'supplementary group is not nobody primary GID'
            done
            groups_seen=1
            printf 'Groups: own-gid-only count=%s\n' "${#groups[@]}"
            ;;
        CapInh:|CapPrm:|CapEff:|CapAmb:)
            [[ $value =~ ^0+$ ]] || fail 'inherited/permitted/effective/ambient capabilities must be zero'
            caps_seen=$((caps_seen + 1))
            printf '%s %s\n' "$key" "$value"
            ;;
    esac
done < /proc/self/status
[[ $uid_seen == 1 && $gid_seen == 1 && $groups_seen == 1 && $caps_seen == 4 ]] \
    || fail 'required kernel credential fields are missing'
[[ -r /proc/self/exe && -x /proc/self/exe ]] || fail 'own proc executable is inaccessible'
readlink -- /proc/self/exe >/dev/null || fail 'own proc executable cannot be resolved'
IFS= read -r boot_id < /proc/sys/kernel/random/boot_id || fail 'boot identity is inaccessible'
[[ $boot_id =~ ^[[:xdigit:]]{8}-[[:xdigit:]]{4}-[[:xdigit:]]{4}-[[:xdigit:]]{4}-[[:xdigit:]]{12}$ ]] \
    || fail 'boot identity is malformed'
for binary in "$runtime_binary" "$launcher_binary"; do
    [[ -f $binary && -r $binary && -x $binary && ! -u $binary && ! -g $binary ]] \
        || fail 'test and launcher binaries must be readable executable files without setuid/setgid'
done
[[ -d $scratch && -r $scratch && -x $scratch ]] || fail 'private scratch is not readable/searchable'
cd -- "$scratch" || fail 'private test working directory is inaccessible'
[[ -d /tmp && -w /tmp && -x /tmp ]] || fail 'scratch parent is unavailable'
printf 'close qualification identity: account=nobody uid=%s gid=%s runner_uid=%s\n' "$uid" "$gid" "$runner_uid"
PREFLIGHT

trap - EXIT
readonly scratch
cleanup() {
    status=$?
    trap - EXIT
    if (( status == 0 )); then
        if ! rm -rf -- "$scratch"; then
            printf 'close qualification: private scratch cleanup failed\n' >&2
        fi
    else
        printf 'close qualification: failure scratch retained; process completion not proven: %s\n' "$scratch" >&2
    fi
    exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
export HOME="$scratch/home" TMPDIR="$scratch/tmp"
export TMUX_TMPDIR="$TMPDIR"
export XDG_CACHE_HOME="$scratch/cache" XDG_CONFIG_HOME="$scratch/config"
export XDG_DATA_HOME="$scratch/data" XDG_STATE_HOME="$scratch/state"
export XDG_RUNTIME_DIR="$scratch/run"
mkdir -- "$HOME" "$TMPDIR" "$XDG_CACHE_HOME" "$XDG_CONFIG_HOME" \
    "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_RUNTIME_DIR"
for directory in "$scratch" "$HOME" "$TMPDIR" "$XDG_CACHE_HOME" "$XDG_CONFIG_HOME" \
    "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_RUNTIME_DIR"; do
    [[ $(stat -c '%a' -- "$directory") == 700 ]] || {
        printf 'close qualification: private directory mode is not 700\n' >&2
        exit 1
    }
done
export PHOENIX_BASH_LAUNCH_TEST_EXE="$launcher_binary"
export PHOENIX_BASH_LAUNCH_TEST_REQUIRED=1
# This fixture creates absolute disposable Git paths; no checkout cwd is needed.
cd -- "$scratch"
# Only this exact Git/Worktree journey is admitted, not detached Bash/Tmux suites.
# The dropped identity owns its inner deadline; workflow adds an emergency bound.
if /usr/bin/timeout --kill-after=10s 5m "$runtime_binary" "$@" < /dev/null; then
    status=0
else
    status=$?
fi
exit "$status"
DROPPED
