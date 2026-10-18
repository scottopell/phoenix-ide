#!/usr/bin/env bash
set -euo pipefail
: "${PHOENIX_CHROME_EXECUTABLE:?owned native qualification requires a real Chrome executable}"
: "${CHROME_QUALIFICATION_OUTPUT:?receipt directory required}"
test -x "$PHOENIX_CHROME_EXECUTABLE"
: "${PHOENIX_TEST_CHROME:?native CLI regressions must not early-return}"
test "$PHOENIX_TEST_CHROME" = 1
mkdir -p "$CHROME_QUALIFICATION_OUTPUT"
unset PHOENIX_SKIP_BROWSER_TESTS
export RUST_TEST_THREADS=1
run_exact() {
  local name="$1"; shift
  cargo test "$@" "$name" -- --exact --nocapture --test-threads=1 2>&1 | tee "$CHROME_QUALIFICATION_OUTPUT/${name##*::}.log"
  grep -F 'test result: ok. 1 passed; 0 failed;' "$CHROME_QUALIFICATION_OUTPUT/${name##*::}.log"
}
# The ignored real-launch test must exec the matched CLI, never the unit-test executable.
export PHOENIX_TEST_CHROME_SHIM_EXECUTABLE="$PWD/target/debug/phoenix-browser"
test -x "$PHOENIX_TEST_CHROME_SHIM_EXECUTABLE"
run_exact session::launch_slot::tests::production_preparation_failure_retires_and_retries_the_same_ide_path -p phoenix-browser --lib
run_exact session::launch_slot::tests::production_driver_opaque_launch_error_remains_a_barrier -p phoenix-browser --lib
run_exact session::launch_slot::tests::cli_profile_policy_is_explicit_and_stable_scope_path_is_unchanged -p phoenix-browser --lib
run_exact session::launch_slot::tests::proven_pre_spawn_failure_can_retire_without_deleting_the_profile -p phoenix-browser --lib
run_exact session::launch_slot::tests::unobservable_launch_error_is_retained_and_refuses_another_spawn_and_close -p phoenix-browser --lib
run_exact session::launch_slot::tests::partial_initialization_cleanup_failure_retains_authority_and_close_retries_same_slot -p phoenix-browser --lib
run_exact session::launch_slot::tests::canceled_launch_waiter_does_not_remove_reservation_or_hide_it_from_retirement -p phoenix-browser --lib
name=session::launch_slot::tests::real_chrome_partial_initialization_retains_child_and_retries_exact_close
cargo test -p phoenix-browser --lib "$name" -- --ignored --exact --nocapture --test-threads=1 2>&1 | tee "$CHROME_QUALIFICATION_OUTPUT/${name##*::}.log"
grep -F 'test result: ok. 1 passed; 0 failed;' "$CHROME_QUALIFICATION_OUTPUT/${name##*::}.log"
run_exact process_group::native::tests::group_query_decoder_distinguishes_empty_error_and_truncation -p phoenix-browser --lib
run_exact process_group::native::tests::linux_scan_skips_only_kernel_proven_nonmembers_without_reading_stat -p phoenix-browser --lib
run_exact process_group::native::tests::linux_scan_reclassifies_missing_candidate_stat_without_assuming_exit -p phoenix-browser --lib
run_exact process_group::native::tests::linux_scan_reclassifies_stat_esrch_without_assuming_exit -p phoenix-browser --lib
run_exact process_group::native::tests::linux_scan_requires_readable_candidate_state_and_rechecks_stat_group -p phoenix-browser --lib
if [[ "$(uname -s)" == Darwin ]]; then
  run_exact process_group::native::tests::owned_empty_group_query_contract_before_and_after_root_reap -p phoenix-browser --lib
fi
run_exact process_group::native::tests::missing_signal_target_requires_reobservation_not_completion -p phoenix-browser --lib
run_exact process_group::native::tests::signal_exit_race_reobserves_owned_child_before_draining -p phoenix-browser --lib
run_exact process_group::native::tests::pending_exit_authorizes_neither_signal_nor_completion -p phoenix-browser --lib
run_exact process_group::native::tests::darwin_pending_exit_preserves_birth_identity_and_requires_missing_group -p phoenix-browser --lib
run_exact process_group::native::tests::exited_root_drains_stderr_closed_writer_before_profile_removal -p phoenix-browser --lib
run_exact process_group::native::tests::reaped_or_mismatched_root_cannot_authorize_old_group_signals -p phoenix-browser --lib
run_exact process_group::native::tests::escaped_writer_unwind_before_identity_cancels_pending_readiness -p phoenix-browser --lib
run_exact process_group::native::tests::escaped_writer_unwind_cleans_owned_fixture_before_release -p phoenix-browser --lib
run_exact process_group::native::tests::group_drain_does_not_claim_an_escaped_writer -p phoenix-browser --lib
run_exact session::profile_marker_tests::launch_group_profile_requires_retained_group_drain_authority -p phoenix-browser --lib
run_exact native_chrome_exec_shim_preserves_arguments_and_clears_internal_request -p phoenix-tools --test browser_cli
run_exact malformed_or_recursive_chrome_exec_request_never_enters_cli -p phoenix-tools --test browser_cli
cargo test -p phoenix_ide --test chrome_exec -- --nocapture --test-threads=1 2>&1 | tee "$CHROME_QUALIFICATION_OUTPUT/server-shim.log"
grep -F 'test result: ok. 2 passed; 0 failed;' "$CHROME_QUALIFICATION_OUTPUT/server-shim.log"
run_exact opaque_initial_launch_failure_status_retains_lifetime_and_barrier -p phoenix-tools --test browser_cli
run_exact partial_initial_launch_failure_status_retains_lifetime_and_barrier -p phoenix-tools --test browser_cli
run_exact dead_chrome_cleanup_failure_remains_visible_until_explicit_close -p phoenix-tools --test browser_cli
run_exact sigterm_during_chrome_startup_cleans_up_the_published_browser -p phoenix-tools --test browser_cli
run_exact lost_chrome_reports_state_loss_and_open_replaces_only_browser_lifetime -p phoenix-tools --test browser_cli
# This test can early-return when Chrome is skipped; require its actual native layout witnesses.
grep -F 'chrome_layout ' "$CHROME_QUALIFICATION_OUTPUT/lost_chrome_reports_state_loss_and_open_replaces_only_browser_lifetime.log"
grep -F 'before-root-kill' "$CHROME_QUALIFICATION_OUTPUT/lost_chrome_reports_state_loss_and_open_replaces_only_browser_lifetime.log"
grep -F 'after-root-kill-before-status' "$CHROME_QUALIFICATION_OUTPUT/lost_chrome_reports_state_loss_and_open_replaces_only_browser_lifetime.log"
