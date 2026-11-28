#!/bin/bash
# A per-test nextest dispatcher, INSIDE check_profile_command.py's measurement.
# Usage: bash close-nextest-wrapper.sh ABSOLUTE_WORKSPACE ORIGINAL_BINARY [ARGS...]
set -euo pipefail

fail() {
    printf 'close nextest wrapper: %s\n' "$1" >&2
    exit 2
}

[[ $# -ge 2 && $1 == /* ]] || fail 'expected absolute workspace and original test command'
workspace=$1
shift
expected_test=${NEXTEST_TEST_NAME-}
# Keep this exact allowlist in sync with close-unprivileged-qualification.sh.
case "$expected_test" in
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
        ;;
    *) exec "$@" ;;
esac

# The library binary ID is exactly the package name, not its bin/integration IDs.
[[ ${NEXTEST_BINARY_ID-} == phoenix_ide ]] || fail 'expected phoenix_ide library test binary'
source_binary=$1
shift
[[ $source_binary == /* && ${source_binary##*/} == phoenix_ide-* ]] \
    || fail 'expected absolute original phoenix_ide test binary'
# Nextest supplies one exact selector and nocapture. Accept argument order changes,
# but never broaden a selector or silently drop flags (ignored/list/filter/etc.).
selector_seen=0
exact_seen=0
nocapture_seen=0
threads_seen=0
for arg in "$@"; do
    case "$arg" in
        "$expected_test")
            (( selector_seen == 0 )) || fail 'duplicate selector'
            selector_seen=1 ;;
        --exact)
            (( exact_seen == 0 )) || fail 'duplicate --exact'
            exact_seen=1 ;;
        --nocapture)
            (( nocapture_seen == 0 )) || fail 'duplicate --nocapture'
            nocapture_seen=1 ;;
        --test-threads=1)
            (( threads_seen == 0 )) || fail 'duplicate thread limit'
            threads_seen=1 ;;
        *) fail 'unexpected libtest argument or mismatched selector' ;;
    esac
done
[[ $selector_seen == 1 && $exact_seen == 1 && $nocapture_seen == 1 ]] \
    || fail 'requires matching single selector, --exact and --nocapture'

# Use the command's archive-extracted binary, NEVER rebuild or substitute it.
# The existing harness checksums/stages it and the launcher, verifies nobody's
# credentials, and owns its existing deadline. Exec preserves status and output;
# the runner-owned profiler remains the parent and writes the same test receipt.
exec bash "$workspace/scripts/ci/close-unprivileged-qualification.sh" \
    "$workspace" "$source_binary" "$workspace/target/debug/phoenix_ide" \
    "$(id -u)" "$expected_test" --exact --nocapture --test-threads=1
