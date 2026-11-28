#!/usr/bin/env bash
set -euo pipefail
test "$(id -u)" -ne 0
runtime_binary="$1"
evidence_dir="$2"
test -x "$runtime_binary"
for test_name in \
  runtime::executor::creation_completion_lifecycle_tests::native_close_admission_tests::worktree_physical::registered_native_close_with_production_detector_retires_worktree_and_replays \
  runtime::executor::creation_completion_lifecycle_tests::native_close_admission_tests::worktree_physical::registered_native_close_refuses_a_live_writable_descriptor \
  runtime::close_retirement::tests::linux_first_descriptor_link_distinguishes_denial_from_identity_loss \
  runtime::close_retirement::tests::linux_writer_scan_keeps_searching_after_an_unavailable_process \
  runtime::close_retirement::tests::linux_mapping_scan_requires_writable_shared_authority; do
  grep -Fx "$test_name: test" "$evidence_dir/runtime-inventory.log"
  (
    cd crates/phoenix-ide
    # Keep an unrelated, same-UID process alive whose /proc/PID/exe
    # cannot be read. No target handle, namespaces, sudo or UID change.
    timeout --kill-after=10s 6m python3 - "$runtime_binary" "$test_name" --exact --show-output --test-threads=1 <<'PYTHON'
import ctypes
import os
import subprocess
import sys

PR_SET_DUMPABLE = 4
libc = ctypes.CDLL(None, use_errno=True)
if libc.prctl(PR_SET_DUMPABLE, 0, 0, 0, 0) != 0:
    raise OSError(ctypes.get_errno(), "cannot create disposable opaque process")
env = dict(os.environ, PHOENIX_TEST_OPAQUE_CLOSE_PID=str(os.getpid()))
raise SystemExit(subprocess.call(sys.argv[1:], env=env))
PYTHON
  ) 2>&1 | tee "$evidence_dir/ordinary-linux-${test_name##*::}.log"
  grep -F "$test_name ... ok" "$evidence_dir/ordinary-linux-${test_name##*::}.log"
  grep -F 'test result: ok. 1 passed; 0 failed; 0 ignored;' "$evidence_dir/ordinary-linux-${test_name##*::}.log"
  if [[ "$test_name" == *::registered_native_close_with_production_detector_retires_worktree_and_replays ]]; then
    grep -F 'ordinary Linux: opaque process witnessed; durable gap precedes removal of the exact checkout and registration' "$evidence_dir/ordinary-linux-${test_name##*::}.log"
  fi
done
