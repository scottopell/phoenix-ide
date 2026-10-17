#!/usr/bin/env bash
set -euo pipefail
: "${PHOENIX_CHROME_EXECUTABLE:?owned native qualification requires a real Chrome executable}"
: "${CHROME_QUALIFICATION_OUTPUT:?receipt directory required}"
test -x "$PHOENIX_CHROME_EXECUTABLE"
mkdir -p "$CHROME_QUALIFICATION_OUTPUT"
unset PHOENIX_SKIP_BROWSER_TESTS
export RUST_TEST_THREADS=1
run_exact() {
  local name="$1"; shift
  cargo test "$@" "$name" -- --exact --nocapture --test-threads=1 2>&1 | tee "$CHROME_QUALIFICATION_OUTPUT/${name##*::}.log"
  grep -F 'test result: ok. 1 passed; 0 failed;' "$CHROME_QUALIFICATION_OUTPUT/${name##*::}.log"
}
run_exact process_group::native::tests::exited_root_drains_stderr_closed_writer_before_profile_removal -p phoenix-browser --lib
run_exact process_group::native::tests::reaped_or_mismatched_root_cannot_authorize_old_group_signals -p phoenix-browser --lib
run_exact process_group::native::tests::group_drain_does_not_claim_an_escaped_writer -p phoenix-browser --lib
run_exact session::profile_marker_tests::launch_group_profile_requires_retained_group_drain_authority -p phoenix-browser --lib
run_exact native_chrome_exec_shim_preserves_arguments_and_clears_internal_request -p phoenix-tools --test browser_cli
run_exact malformed_or_recursive_chrome_exec_request_never_enters_cli -p phoenix-tools --test browser_cli
cargo test -p phoenix_ide --test chrome_exec -- --nocapture --test-threads=1 2>&1 | tee "$CHROME_QUALIFICATION_OUTPUT/server-shim.log"
grep -F 'test result: ok. 2 passed; 0 failed;' "$CHROME_QUALIFICATION_OUTPUT/server-shim.log"
run_exact lost_chrome_reports_state_loss_and_open_replaces_only_browser_lifetime -p phoenix-tools --test browser_cli
# This test can early-return when Chrome is skipped; require its actual native layout witnesses.
grep -F 'chrome_layout ' "$CHROME_QUALIFICATION_OUTPUT/lost_chrome_reports_state_loss_and_open_replaces_only_browser_lifetime.log"
grep -F 'before-root-kill' "$CHROME_QUALIFICATION_OUTPUT/lost_chrome_reports_state_loss_and_open_replaces_only_browser_lifetime.log"
grep -F 'after-root-kill-before-status' "$CHROME_QUALIFICATION_OUTPUT/lost_chrome_reports_state_loss_and_open_replaces_only_browser_lifetime.log"
