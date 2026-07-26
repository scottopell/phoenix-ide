# Isolate browser test sessions and make shutdown authoritative

## Failure

Browser CI tests fail intermittently with unrelated symptoms: missing DOM nodes, stale checkbox state, empty titles, browser-init timeouts, and launch/profile errors. Reruns commonly recover.

## Evidence

`crates/phoenix-tools/src/browser/tests.rs::test_context` gives every test a fresh manager but the same durable `WorkScopeId("test-work")`. `BrowserSessionManager` derives Chrome's deterministic user-data directory from that scope, so parallel tests and independent managers race on `/tmp/phoenix-chrome-c82621c8f18b0fee`.

The fixture calls `BrowserSessionManager::shutdown_all`, but that method only clears the session map. `Drop for BrowserSession` aborts async tasks without explicitly closing or killing Chrome. A local process snapshot found 15 orphaned Chrome helpers, all reparented to PID 1 and all using the exact `work:test-work` profile hash.

Open PR #588 intentionally excludes `shutdown_all` correctness and preserves profile identity, so it does not own this fix.

## Plan

1. Give each browser test context a unique WorkScope so parallel tests cannot share a session/profile accidentally.
2. Make `shutdown_all` terminate every owned Chrome session and await profile cleanup rather than clearing bookkeeping only.
3. Add focused regressions for unique fixture identity and authoritative multi-session shutdown where observable without timing sleeps.
4. Preserve production WorkScope sharing and deterministic profile reuse.

## Validation

- Reproduce/profile the pre-fix parallel collision under the `./dev.py` browser environment classification.
- Run focused browser tests repeatedly and in parallel.
- Run owning-crate tests and `./dev.py check`.
- Confirm no newly launched Phoenix Chrome processes/profile locks remain after the test run.
