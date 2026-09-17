# Repair tmux test watchdog process containment

## Observed journey

- Real-server tmux tests create a test-owned tmux server and pane shell under a unique `/private/tmp/ptt-*` root through `TestTmuxServerOwner`.
- Against the exact `WATCHDOG_PROGRAM`, ordinary cleanup kills the server and shell and removes the root. If the live Unix socket is unlinked before cleanup is requested, the watchdog exits successfully and removes the root while both the tmux server and pane shell remain alive.
- This is an isolated current-`main` test-containment repair. It is not evidence for the original September 9 Close blocker, and filesystem generated-bulk reclamation is not lifecycle completion.

## Reproduction and environment

- Implement from current `origin/main`; the investigation inspected `6185f1501930ed6fa999b3c9aadb9d6992ee4076` (2026-09-16).
- Preserved read-only RCA artifacts:
  - `$HOME/.phoenix-ide/incidents/close-finalize-20260916/reproduce-watchdog.py` — SHA-256 `8effdb15b209d0b42d1d732137d33c8e36d9e9650cd0f80bdac65896630676db`
  - `$HOME/.phoenix-ide/incidents/close-finalize-20260916/watchdog-reproduction-results.json` — SHA-256 `7ecd19f7c656470a0b185642f7fae918c47c868034897b77ef7d48348086f4e0`
  - `$HOME/.phoenix-ide/incidents/close-finalize-20260916/leaked-test-processes.json` — SHA-256 `d441412186bc85674ae4460665e1c3feea13735ad5ec7c704ab083a6ca8745f6`
- GitHub Issue #651 could not be refreshed during Explore because network access was sandbox-blocked. Requirements, current `origin/main`, local history, and the relayed incident artifacts were used as authorities instead.

## Verified findings

- `crates/phoenix-tools/src/tmux/test_server.rs::WATCHDOG_PROGRAM` discovers owned servers only by globbing live `*.sock` filesystem entries. Its five-quiet-probe success condition therefore has no process identity to check after an endpoint or root disappears.
- `TestTmuxServerOwner::finish` treats watchdog exit status 0 as success and only calls `verify_no_live_servers` when the root still exists. Once the watchdog removes the root, neither path verifies that a previously owned server or pane process exited.
- The reproduced `missing_socket` case recorded watchdog exit 0 and `root_exists: false`, while server PID 15743 and pane-shell PID 15744 retained the same `ps` identities after cleanup. The control case killed both processes.
- The `missing_root` case exceeded the reproduction script's bounded wait and has no recorded watchdog outcome. Treat it only as an uncovered/liveness edge to repair and test; do not claim the current watchdog falsely succeeds in that case.
- `leaked-test-processes.json` records nine PID-1-owned tmux servers, each with a surviving shell child. Every command uses socket name `wt-a646665f6b041ce8.sock`; `a646665f6b041ce8` is the first 16 hex characters of SHA-256(`unlinked-live-server`).
- Current-main `registry.rs::unlinked_live_server_is_not_accepted_as_absent` deliberately creates that work-scope name, starts a live contained server, unlinks its socket, verifies production retirement fails closed with `IdentityNotProven`, and then calls `owner.shutdown()`.
- The watchdog source SHA-256 is `27f085decf3361829893f696292c7fd94d21f757ee240c766895bf700cb5ca4c`. It is byte-identical in the original #590 commit (`679736e83655d911ece1162baaa767190614bb4f`), the retained reproduction source identified by the incident script, and current `origin/main`.
- #590's completed task required exact run-owned cleanup, abrupt-owner-death containment, surfaced cleanup failure, and structural exclusion of durable production sockets. The present endpoint-loss hole violates those test-harness obligations.
- Normative production behavior remains fail-closed: tmux Close/Delete retirement must not infer exact-server absence from a missing or ambiguous endpoint. The current regression correctly preserves the registry entry when identity cannot be proven.

## Failure model

The fixture owns a filesystem namespace but does not retain an independently verifiable identity for every tmux server process created in that namespace. Socket disappearance therefore erases the watchdog's only discovery handle. Cleanup can mistake “no discoverable socket” for “no owned process,” remove the root, and report success while the daemonized server and its pane shell retain the deleted test directory as their environment/cwd.

The repair must make test process ownership independent of socket and temp-directory lifetime. It must not weaken production retirement or infer process ownership from a PID, process name, socket-name hash, cwd string, or broad process scan alone.

## Interaction map

- Test registry spawn (`TmuxRegistry::with_test_spawn_containment` / `spawn_session_owned`) → bounded handoff of exact spawned-server identity to the test owner/watchdog.
- Test owner/watchdog → exact identity verification → bounded termination of only that owned tmux server (and consequently its pane shell) → verified process exit → root removal and successful cleanup result.
- `Drop`, explicit `shutdown`, Tokio cancellation, panic unwind, and abrupt test-owner death all converge on the same ownership record and verification contract.
- Socket unlink and temp-root removal are adverse inputs, not proof of process absence.
- Production registries and production Close retirement must remain outside this test-only ownership path.

## Proposed scope

### Owning invariant

For every tmux server spawned through `TestTmuxServerOwner`, the harness captures a bounded, exact, verifiable process identity before reporting spawn success. Cleanup terminates and verifies the exit of that exact server and its pane shell independently of socket or temp-root lifetime. Cleanup must fail loudly and must not report success while an owned process remains.

### Implementation surfaces

- Repair the test-only containment protocol around:
  - `crates/phoenix-tools/src/tmux/test_server.rs::TestTmuxServerOwner`
  - `WATCHDOG_PROGRAM`
  - the contained-spawn handoff in `crates/phoenix-tools/src/tmux/registry.rs` (`tmux_spawn_command`, creator markers/gate, and `spawn_session_owned`)
- Capture sufficient birth/identity evidence at spawn time and validate it again before signaling or killing, so PID reuse or an altered marker cannot target an unrelated process.
- Keep ownership bounded to the unique test root and server created through the test-only registry. Prefer an exact process handle or PID-plus-birth/token relationship over later filesystem rediscovery.
- Verify both the exact tmux server and its owned pane shell are gone before success. Preserve diagnostic evidence and fail the test when identity or termination cannot be proven.
- Preserve structural separation (`contain_test_spawns` / test-support path) so production spawn, persistence, and Close/Delete behavior are unchanged.

Do not prescribe arbitrary process killing or a global orphan scan. If a platform cannot prove that a live process is the captured test-owned identity, cleanup must fail closed rather than signal it.

## Regression matrix and acceptance evidence

Add deterministic, bounded regressions covering:

1. Normal explicit shutdown.
2. Panic/unwind.
3. Tokio task cancellation.
4. Abrupt test-owner process death, including the existing direct-process and process-group cases.
5. Live socket unlinked before cleanup.
6. Temp root removed while the server and owner/watchdog are live.

For every applicable case:

- capture the server and pane-shell identities before the lifecycle edge;
- assert the exact server exits;
- assert the exact pane shell exits;
- assert no surviving owned process retains the test cwd/root;
- assert cleanup does not return success until those postconditions are verified;
- assert unrelated and durable Phoenix tmux servers cannot be selected or killed;
- use bounded waits and retain actionable evidence on failure.

First reproduce the endpoint-loss regression with a focused test, then make the matrix pass. Run focused `phoenix-tools` tmux tests and the relevant cross-crate real-server tests. Defer repeated/parallel stress and full `./dev.py check` until the focused repair is correct; no heavy validation is part of the pre-approval phase.

## Risks

- PID reuse or stale/tampered identity records could cause unrelated process termination unless identity includes a birth/token check and remains tied to the unique owner.
- Tmux daemonization separates the server from the short-lived creator wrapper; capturing the wrapper alone is insufficient.
- Deleting the root can erase file-backed identity state, so the watchdog needs identity material that survives that edge.
- Platform process APIs differ between macOS and Linux; unsupported proof must fail closed without broadening the kill target.
- Waiting only for the server can miss a retained shell, while treating a transient zombie as live can make cleanup flaky. Tests need exact, bounded process-state semantics.

## Explicit non-goals

- No production Close, finalize, retirement, discovery, or tmux lifecycle behavior changes.
- No change to `unlinked_live_server_is_not_accepted_as_absent`'s fail-closed production assertion except any test-only observation needed to prove cleanup.
- No claim that this explains or repairs the original September 9 Close blocker.
- No merge, deploy, production restart, historical orphan cleanup, generated-bulk reclamation, or retained-source archive work.
- No global process-name/socket-hash/cwd scan and no killing processes not proven to belong to the exact test owner.
- No normative tmux product-spec change unless implementation reveals an actual production contract change; this task is test infrastructure containment.
