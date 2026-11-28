#!/bin/bash
# Usage: bash close-unprivileged-qualification.sh WORKSPACE BINARY LAUNCHER_OR_HTTP_ARCHIVE RUNNER_UID SELECTOR [OPTIONS...]
# Build as the runner first; tee evidence outside this wrapper. Hosted CI supplies the outer deadline.
set -euo pipefail

fail() {
    printf 'close qualification: %s\n' "$1" >&2
    exit 1
}

[[ $# -ge 5 ]] || fail 'expected workspace, test binary, launcher, runner UID, and test selector/options'
[[ $1 == /* && $2 == /* && $3 == /* ]] || fail 'workspace and binary paths must be absolute'
[[ $4 =~ ^[1-9][0-9]*$ ]] || fail 'runner UID must be numeric and nonroot'
[[ $UID == "$4" && $EUID == "$4" ]] || fail 'caller must be the specified nonroot runner'
# Namespace creation needs privilege and is intentionally confined to disposable,
# GitHub-hosted Linux runners. Do not permit local or self-hosted fixture setup.
[[ ${GITHUB_ACTIONS-} == true && ${RUNNER_ENVIRONMENT-} == github-hosted && ${RUNNER_OS-} == Linux ]] \
    || fail 'privileged namespace setup is restricted to GitHub-hosted Linux runners'

workspace=$1
source_runtime_binary=$2
source_launcher_binary=$3
runner_uid=$4
shift 4
# Admit only exact disposable cases, never a suite prefix or an arbitrary command.
case ${1-} in
    git_lifecycle_public_http)
        [[ $# == 3 && $2 =~ ^[0-9a-f]{64}$ && $3 =~ ^[0-9a-f]{64}$ ]] || exit 2
        http_mode=1 ;;
    runtime::executor::creation_completion_lifecycle_tests::native_close_admission_tests::worktree_physical::registered_native_close_with_production_detector_retires_worktree_and_replays | \
    runtime::executor::creation_completion_lifecycle_tests::native_close_admission_tests::worktree_physical::registered_native_close_captured_loss_public_retry_with_production_detector_reclaims_and_replays | \
    runtime::executor::creation_completion_lifecycle_tests::native_close_admission_tests::worktree_physical::registered_native_close_captured_loss_retry_final_writer_gap_preserves_checkout | \
    runtime::executor::creation_completion_lifecycle_tests::native_close_admission_tests::worktree_physical::registered_native_close_captured_loss_retry_stop_during_assessment_preserves_worktree_and_history | \
    runtime::executor::creation_completion_lifecycle_tests::native_close_admission_tests::worktree_physical::registered_native_close_captured_loss_retry_stop_while_prepared_preserves_worktree | \
    runtime::executor::creation_completion_lifecycle_tests::native_close_admission_tests::worktree_physical::registered_native_close_captured_loss_retry_stop_before_writer_refuses_dispatch | \
    runtime::executor::creation_completion_lifecycle_tests::native_close_admission_tests::worktree_physical::registered_native_close_captured_loss_retry_stop_before_physical_observation_retains_stopped_result | \
    runtime::executor::creation_completion_lifecycle_tests::native_close_admission_tests::worktree_physical::registered_native_close_captured_loss_retry_stop_during_physical_mutation_does_not_revalidate_pending_ticks | \
    runtime::executor::creation_completion_lifecycle_tests::native_close_admission_tests::worktree_physical::registered_native_close_captured_loss_retry_resumes_physical_mutation_after_pending_ticks | \
    runtime::close_retirement::tests::anchored_native_retry_assessment_preserves_writer_inspection_gap | \
    runtime::close_retirement::tests::quarantine_rejects_live_linux_descriptor_writer_before_deletion)
        expected_test=$1
        http_mode=0 ;;
    *) echo 'identity wrapper refuses a selector outside disposable Worktree qualification' >&2; exit 2 ;;
esac
[[ $http_mode == 1 || ( $# == 4 && $1 == "$expected_test" && $2 == --exact && $3 == --nocapture && $4 == --test-threads=1 ) ]] \
    || fail 'identity wrapper requires one allowed exact disposable Worktree journey and fixed options'

for artifact in "$source_runtime_binary" "$source_launcher_binary"; do
    [[ -f $artifact && -r $artifact && ! -u $artifact && ! -g $artifact ]] \
        || fail 'source artifacts must be readable regular files without setuid/setgid'
done
[[ -x $source_runtime_binary && ( $http_mode == 1 || -x $source_launcher_binary ) ]] \
    || fail 'source binaries must be executable'

# Stream the already-built artifacts into a nobody-owned private directory. This
# changes no checkout permissions and does not invoke a build as the dropped user.
scratch=$(/usr/bin/sudo -n -u nobody -- /usr/bin/mktemp -d /tmp/phoenix-close-qualification.XXXXXXXXXX) \
    || fail 'could not create nobody-owned private scratch'
caller_pid_ns=$(/usr/bin/stat -Lc '%i' /proc/self/ns/pid) \
    || fail 'could not capture caller PID namespace identity'
cleanup_staging() {
    status=$?
    trap - EXIT
    if ! /usr/bin/sudo -n -u nobody -- /bin/rm -rf -- "$scratch"; then
        printf 'close qualification: staging cleanup failed\n' >&2
        status=1
    fi
    exit "$status"
}
trap cleanup_staging EXIT
stage_artifact() {
    local source=$1 destination=$2 mode=$3 expected=${4-} actual
    if [[ -z $expected ]]; then
        expected=$(/usr/bin/sha256sum -- "$source" | /usr/bin/cut -d ' ' -f 1) \
            || fail 'could not checksum source artifact'
    fi
    /bin/cat -- "$source" | /usr/bin/sudo -n -u nobody -- /usr/bin/tee -- "$destination" >/dev/null \
        || fail 'could not stream artifact into nobody-owned scratch'
    /usr/bin/sudo -n -u nobody -- /bin/chmod "$mode" -- "$destination" \
        || fail 'could not set staged artifact mode'
    actual=$(/usr/bin/sudo -n -u nobody -- /usr/bin/sha256sum -- "$destination" | /usr/bin/cut -d ' ' -f 1) \
        || fail 'could not checksum staged artifact'
    [[ $expected == "$actual" ]] || fail 'staged artifact checksum differs from expected digest'
    printf 'close qualification: staged %s sha256=%s\n' "${destination##*/}" "$actual"
}
if [[ $http_mode == 1 ]]; then
    stage_artifact "$source_runtime_binary" "$scratch/runtime-test" 700 "$2"
    stage_artifact "$source_launcher_binary" "$scratch/http-payload.tar" 600 "$3"
    auxiliary="$scratch/http-payload.tar"
    set -- git_lifecycle_public_http
else
    stage_artifact "$source_runtime_binary" "$scratch/runtime-test" 700
    stage_artifact "$source_launcher_binary" "$scratch/bash-launcher" 700
    auxiliary="$scratch/bash-launcher"
fi

# Namespace teardown is coupled to its supervisor via --kill-child. Remove
# staged inputs only after unshare exits.
if /usr/bin/sudo -n -- /usr/bin/env -i PATH=/usr/bin:/bin LC_ALL=C \
    /usr/bin/timeout --kill-after=5s 75s \
    /usr/bin/unshare --kill-child=KILL --mount --pid --fork --mount-proc \
    /bin/bash --noprofile --norc -s -- "$scratch" "$workspace" "$scratch/runtime-test" "$auxiliary" "$runner_uid" "$caller_pid_ns" "$@" <<'PID1'
set -euo pipefail
scratch=$1 workspace=$2 runtime_binary=$3 launcher_binary=$4 runner_uid=$5 caller_pid_ns=$6
shift 6
[[ $$ == 1 ]] || { echo 'namespace supervisor is not PID 1' >&2; exit 1; }
private_pid_ns=$(/usr/bin/stat -Lc '%i' /proc/self/ns/pid)
[[ $private_pid_ns != "$caller_pid_ns" ]] || { echo 'PID namespace was not isolated from caller' >&2; exit 1; }
[[ $(/usr/bin/stat -Lc '%i' /proc/1/ns/pid) == "$private_pid_ns" ]] \
    || { echo 'PID 1 does not belong to private PID namespace' >&2; exit 1; }
/bin/mount --make-rprivate /
uid=$(/usr/bin/id -u nobody)
gid=$(/usr/bin/id -g nobody)
/bin/mkdir -- "$scratch/private"
/bin/mount -t tmpfs -o "mode=700,uid=$uid,gid=$gid,nosuid,nodev,noexec,size=256m" tmpfs "$scratch/private"
# Credential changes clear the kernel parent-death signal. For HTTP, keep
# namespace PID 1 as the setup supervisor; only its child drops credentials.
launch_dropped() {
    if [[ $1 == git_lifecycle_public_http ]]; then
        shift
        local child_status=0
        "$@" || child_status=$?
        return "$child_status"
    else
        shift
        exec "$@"
    fi
}
launch_dropped "$1" /usr/bin/setpriv --reuid "$uid" --regid "$gid" --clear-groups \
    --inh-caps=-all --ambient-caps=-all --bounding-set=-all --no-new-privs \
    /usr/bin/env -i PATH=/usr/bin:/bin LC_ALL=C USER=nobody LOGNAME=nobody \
    /bin/bash --noprofile --norc -s -- "$workspace" "$runtime_binary" "$launcher_binary" "$runner_uid" "$scratch" "$caller_pid_ns" "$private_pid_ns" "$@" <<'DROPPED'
set -euo pipefail
umask 077
trap 'exit 130' INT
trap 'exit 143' TERM
workspace=$1
runtime_binary=$2
launcher_binary=$3
runner_uid=$4
scratch=$5
caller_pid_ns=$6
private_pid_ns=$7
shift 7
# Admit only exact disposable cases, never a suite prefix or an arbitrary command.
case ${1-} in
    git_lifecycle_public_http)
        [[ $# == 1 ]] || exit 2
        http_mode=1 ;;
    runtime::executor::creation_completion_lifecycle_tests::native_close_admission_tests::worktree_physical::registered_native_close_with_production_detector_retires_worktree_and_replays | \
    runtime::executor::creation_completion_lifecycle_tests::native_close_admission_tests::worktree_physical::registered_native_close_captured_loss_public_retry_with_production_detector_reclaims_and_replays | \
    runtime::executor::creation_completion_lifecycle_tests::native_close_admission_tests::worktree_physical::registered_native_close_captured_loss_retry_final_writer_gap_preserves_checkout | \
    runtime::executor::creation_completion_lifecycle_tests::native_close_admission_tests::worktree_physical::registered_native_close_captured_loss_retry_stop_during_assessment_preserves_worktree_and_history | \
    runtime::executor::creation_completion_lifecycle_tests::native_close_admission_tests::worktree_physical::registered_native_close_captured_loss_retry_stop_while_prepared_preserves_worktree | \
    runtime::executor::creation_completion_lifecycle_tests::native_close_admission_tests::worktree_physical::registered_native_close_captured_loss_retry_stop_before_writer_refuses_dispatch | \
    runtime::executor::creation_completion_lifecycle_tests::native_close_admission_tests::worktree_physical::registered_native_close_captured_loss_retry_stop_before_physical_observation_retains_stopped_result | \
    runtime::executor::creation_completion_lifecycle_tests::native_close_admission_tests::worktree_physical::registered_native_close_captured_loss_retry_stop_during_physical_mutation_does_not_revalidate_pending_ticks | \
    runtime::executor::creation_completion_lifecycle_tests::native_close_admission_tests::worktree_physical::registered_native_close_captured_loss_retry_resumes_physical_mutation_after_pending_ticks | \
    runtime::close_retirement::tests::anchored_native_retry_assessment_preserves_writer_inspection_gap | \
    runtime::close_retirement::tests::quarantine_rejects_live_linux_descriptor_writer_before_deletion)
        expected_test=$1
        http_mode=0 ;;
    *) echo 'identity wrapper refuses a selector outside disposable Worktree qualification' >&2; exit 2 ;;
esac
[[ $http_mode == 1 || ( $# == 4 && $1 == "$expected_test" && $2 == --exact && $3 == --nocapture && $4 == --test-threads=1 ) ]] || {
    echo 'identity wrapper requires one allowed exact disposable Worktree journey and fixed options' >&2
    exit 2
}

# Bound preflight independently of the same-identity test deadline below.
/usr/bin/timeout --kill-after=1s 10s /bin/bash --noprofile --norc -s -- \
    "$scratch" "$runtime_binary" "$launcher_binary" "$runner_uid" "$caller_pid_ns" "$private_pid_ns" "$http_mode" <<'PREFLIGHT'
set -euo pipefail
fail() {
    printf 'close qualification preflight: %s\n' "$1" >&2
    exit 1
}
scratch=$1
private_storage="$scratch/private"
runtime_binary=$2
launcher_binary=$3
runner_uid=$4
caller_pid_ns=$5
private_pid_ns=$6
http_mode=$7
for tool in bash git sleep timeout tmux id readlink stat mktemp mkdir rm awk; do
    command -v "$tool" >/dev/null || fail 'required system tool is missing'
done
uid=$(id -u nobody)
gid=$(id -g nobody)
[[ $uid =~ ^[1-9][0-9]*$ && $gid =~ ^[1-9][0-9]*$ ]] || fail 'nobody must have a nonroot numeric UID and GID'
[[ $UID == "$uid" && $EUID == "$uid" && $uid != "$runner_uid" ]] || fail 'selected identity is not the distinct nobody account'
[[ $(id -un) == nobody && $(id -g) == "$gid" ]] || fail 'selected account or primary group does not match nobody'
[[ $(awk '/^NoNewPrivs:/ { print $2 }' /proc/self/status) == 1 ]] || fail 'no_new_privs is not set'
[[ $(awk '/^CapBnd:/ { print $2 }' /proc/self/status) =~ ^0+$ ]] || fail 'capability bounding set is not empty'

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
            [[ -z $value ]] || fail 'supplementary groups must be empty'
            groups_seen=1
            printf 'Groups: empty\n'
            ;;
        CapInh:|CapPrm:|CapEff:|CapAmb:|CapBnd:)
            [[ $value =~ ^0+$ ]] || fail 'inherited/permitted/effective/ambient/bounding capabilities must be zero'
            caps_seen=$((caps_seen + 1))
            printf '%s %s\n' "$key" "$value"
            ;;
    esac
done < /proc/self/status
[[ $uid_seen == 1 && $gid_seen == 1 && $groups_seen == 1 && $caps_seen == 5 ]] \
    || fail 'required kernel credential fields are missing'
[[ -r /proc/self/exe && -x /proc/self/exe ]] || fail 'own proc executable is inaccessible'
readlink -- /proc/self/exe >/dev/null || fail 'own proc executable cannot be resolved'
[[ $private_pid_ns != "$caller_pid_ns" ]] || fail 'PID namespace matches caller namespace'
[[ $(stat -Lc '%i' /proc/self/ns/pid) == "$private_pid_ns" ]] || fail 'private PID namespace identity changed'
if [[ $http_mode == 1 ]]; then
    # The setup supervisor checked the namespace link before dropping the child.
    [[ $(awk '/^Pid:/ { print $2 }' /proc/1/status) == 1 ]] || fail 'private PID 1 is missing'
else
    [[ $(stat -Lc '%i' /proc/1/ns/pid) == "$private_pid_ns" ]] || fail 'PID 1 does not belong to private PID namespace'
fi
[[ $(stat -f -c %T -- "$scratch/private") == tmpfs ]] || fail 'fixture storage is not private tmpfs'
IFS= read -r boot_id < /proc/sys/kernel/random/boot_id || fail 'boot identity is inaccessible'
[[ $boot_id =~ ^[[:xdigit:]]{8}-[[:xdigit:]]{4}-[[:xdigit:]]{4}-[[:xdigit:]]{4}-[[:xdigit:]]{12}$ ]] \
    || fail 'boot identity is malformed'
for artifact in "$runtime_binary" "$launcher_binary"; do
    [[ -f $artifact && -r $artifact && ! -u $artifact && ! -g $artifact ]] \
        || fail 'staged artifacts must be readable regular files without setuid/setgid'
done
[[ -x $runtime_binary && ( $http_mode == 1 || -x $launcher_binary ) ]] \
    || fail 'staged binaries must be executable'
[[ -d $scratch && -r $scratch && -x $scratch ]] || fail 'private scratch is not readable/searchable'
[[ $(stat -c '%a' -- "$private_storage") == 700 ]] || fail 'private storage mode is not 700'
cd -- "$scratch" || fail 'private test working directory is inaccessible'
[[ -d /tmp && -w /tmp && -x /tmp ]] || fail 'scratch parent is unavailable'
printf 'close qualification identity: account=nobody uid=%s gid=%s runner_uid=%s no_new_privs=1\n' "$uid" "$gid" "$runner_uid"
printf 'close qualification namespaces: caller_pid=%s private_pid=%s storage=tmpfs\n' "$caller_pid_ns" "$private_pid_ns"
PREFLIGHT

trap - EXIT
readonly scratch
trap 'exit 130' INT
trap 'exit 143' TERM
private_storage="$scratch/private"
export HOME="$private_storage/home" TMPDIR="$private_storage/tmp"
export TMUX_TMPDIR="$TMPDIR"
export XDG_CACHE_HOME="$private_storage/cache" XDG_CONFIG_HOME="$private_storage/config"
export XDG_DATA_HOME="$private_storage/data" XDG_STATE_HOME="$private_storage/state"
export XDG_RUNTIME_DIR="$private_storage/run"
mkdir -- "$HOME" "$TMPDIR" "$XDG_CACHE_HOME" "$XDG_CONFIG_HOME" \
    "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_RUNTIME_DIR"
for directory in "$scratch" "$private_storage" "$HOME" "$TMPDIR" "$XDG_CACHE_HOME" "$XDG_CONFIG_HOME" \
    "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_RUNTIME_DIR"; do
    [[ $(stat -c '%a' -- "$directory") == 700 ]] || {
        printf 'close qualification: private directory mode is not 700\n' >&2
        exit 1
    }
done
cd -- "$scratch"
if [[ $http_mode == 1 ]]; then
    # Archive extraction and every payload action run only as the dropped UID.
    payload="$private_storage/http"
    mkdir -- "$payload"
    /usr/bin/tar --extract --file "$launcher_binary" --directory "$payload" --no-same-owner --no-same-permissions
    export PYTHONPATH="$payload:$payload/deps" PYTHONNOUSERSITE=1
    export E2E_RUST_LOG=warn,phoenix_ide::runtime::close_retirement=debug
    /usr/bin/timeout --kill-after=5s 70s /usr/bin/python3 -B -P -u "$payload/run.py" --isolated-git-lifecycle < /dev/null
    exit
fi
export PHOENIX_BASH_LAUNCH_TEST_EXE="$launcher_binary"
export PHOENIX_BASH_LAUNCH_TEST_REQUIRED=1
# Only the allowlisted exact Git/Worktree journeys are admitted, not Bash/Tmux suites.
# The dropped identity owns its inner deadline; workflow adds an emergency bound.
if /usr/bin/timeout --kill-after=5s 70s "$runtime_binary" "$@" < /dev/null; then
    status=0
else
    status=$?
fi
exit "$status"
DROPPED
PID1
then
    status=0
else
    status=$?
fi
printf 'close qualification: namespace supervisor returned status=%s\n' "$status"
exit "$status"
