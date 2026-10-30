# Standalone Browser CLI — Executive Summary

## Requirements Summary

Coding agents can use Phoenix's browser operations through a standalone executable, retaining browser state across separate shell commands. Session selection supports an explicit name, an environment binding, and a worktree-based fallback. Navigation starts fresh state when needed; other browser actions report state loss. Navigation supplies accessibility targets and debugging evidence, while profiling preserves the browser tool's measurement contract.

## Technical Summary

The CLI reuses the browser-specific operation boundary from `phoenix-tools` and the CDP engine in `phoenix-browser`. A user-private Unix socket connects short-lived callers to a session daemon. An operating-system startup lock prevents competing daemon owners. A serialized worker executes operations while the daemon independently accepts shutdown. Browser initialization settles under ownership before cancellation; admitted operations can be interrupted with an explicit partial-side-effects warning. The daemon owns browser termination, transient profile cleanup, and a configurable idle interval. Session selection, daemon lifetime, and browser lifetime have separate identities. The local transport provides no cross-version interoperability promise.

The lifecycle contract is specified in `browser-cli.allium`; browser operation semantics remain in `specs/browser-tool/`.

## Scenario queue admission — focused qualification passed

The archive candidate registers a validated `run_scenario` with the broker before the daemon's serial queue. A one-shot permit moves with the queued job into shared profile execution; registration, execution, and cleanup use the same broker task, without a second busy flag or parallel operation worker. Non-scenario serialization and daemon-owned execution after CLI disconnection remain intact.

Added coverage (not executed in this candidate):
- `concurrent_socket_scenarios_reject_before_fixture_release_without_replaying`: real daemon socket plus CLI; an HTTP fixture holds the first scenario until the second returns typed `busy`; verifies no second side effect, recovery, and the first client's disconnection variant.
- `socket_scenario_registration_rejects_competitor_before_serial_dispatch`: real socket request admission before worker dispatch, typed `busy` and `invalid_input`, invalid requests without custody, and successful dispatch/recovery. This white-box test deliberately controls dispatch; it does not exercise a full worker queue under load.
- Broker reservation unit tests cover exclusion before dispatch, abandonment, and close before dispatch.

The sole host reported strict Clippy and 50 broker/native/daemon tests passing on exact v20 `45a40c5f4d277860e55ef4474120521aaf4082c4`. Exact v23 `947bf516c8a61f8a99a757889abe82f10398ad65` changes only the CLI fixture and passed strict Clippy plus the concurrent socket/CLI regression and touch composition test (1 passed/0 failed/0 ignored each). This is split qualification, not 52 tests executed on v23. The same-origin held-response fixture covers competitor rejection and the disconnected-client branch; an elapsed client read-timeout variant is not independently exercised. Receipt hashes and preserved failure attribution are recorded in the [browser-tool executive](../browser-tool/executive.md). All 3392 source hashes matched and execution custody was released; hosted successor CI is separate.

## Status Summary

| Requirement | Status | Notes |
| --- | --- | --- |
| REQ-BCLI-001: Standalone Operation | ✅ Complete | Installed executable smoke: help, schema, open, eval, close without Phoenix. |
| REQ-BCLI-002: Predictable Session Selection | ✅ Complete | Explicit/environment precedence, canonical paths, clone and linked-worktree boundaries; lossless Unix path bytes. |
| REQ-BCLI-003: Exclusive Local Ownership | ✅ Complete | Concurrent initial opens share one owner; named sessions isolate pages. |
| REQ-BCLI-004: Lazy Startup and State Loss | ✅ Complete | Absent discovery, idle expiry, dead-Chrome detection and fresh open. |
| REQ-BCLI-005: Distinct Lifetimes | ✅ Complete | Daemon/browser identities appear on operation success and failure; browser-only replacement rotates identity. |
| REQ-BCLI-006: Idle Expiry and Explicit Shutdown | ✅ Complete | Active work survives idle; close interrupts blocked eval; startup SIGTERM retains cleanup ownership; immediate reopen and descriptor isolation. |
| REQ-BCLI-007: Actionable Navigation Result | ✅ Complete | URL/title/interactive refs, partial snapshot failure, isolated console source fences with main-document and iframe errors. |
| REQ-BCLI-008: Complete Browser Operation Surface | ✅ Complete | Shared operation catalog; real-browser interaction, console, screenshot, scenario, CPU capture and saved summary journey. |
| REQ-BCLI-009: File-Only Discovery and Analysis | ✅ Complete | Browser-free schema/help/saved analysis; malformed startup idle configuration does not block these commands. |
| REQ-BCLI-010: Predictable Output and Errors | ✅ Complete | JSON envelopes, typed error codes, structured measurement failures, bounded stdin, semantic preflight and exit statuses. |
| REQ-BCLI-011: No Ambiguous Operation Replay | ✅ Complete | Missing/malformed response reports transport uncertainty without starting or replaying another request. |
| REQ-BCLI-012: Harness Usage Guidance | ✅ Complete | [Usage and installation](../../docs/phoenix-browser.md), including session binding and profiling. |

**Progress:** 12 of 12 implemented and covered. PR review and final CI qualification are tracked in [PR #925](https://github.com/scottopell/phoenix-ide/pull/925).

## Native teardown qualification candidate

The source candidate isolates Chrome through the existing CLI/server executable's
native re-exec entry, without changing chromiumoxide or rebuilding its argument
list. Linux/macOS launch-group ownership retains the exact root as an unreaped
child (`waitid(WNOWAIT)`), observes status without `try_wait`, and bounds TERM →
KILL → group drain before root reap and profile deletion. Binaries explicitly
install the shim capability; libtest/other embedders do not infer it from
`current_exe`. Other platforms retain the direct-launch path and cannot install
the Unix capability.

**Not an acceptance or all-descendants claim:** the barrier covers members of the
launch group, not a helper that calls `setsid`/`setpgid`. A deterministic escaped
writer negative demonstrates that limitation. The real lost-Chrome regression
records bounded profile/ancestry-selected process layout and rejects observed
escaped helpers before qualifying cleanup. Exact Linux and macOS compiled tests
and actual Chrome layout remain required; source formatting is not runtime proof.
A supplementary retained macOS binary observation is not exact-candidate evidence.

Pre-session retention source candidate (ADR-123, REQ-BCLI-006a): the manager reserves one typed slot before a potentially spawning call and retains returned Browser/handler/group/profile authority across partial initialization and cleanup errors. Close retries that aggregate. Caller cancellation does not own the initialization/listener driver. Retirement captures pending slots by immutable reservation identity rather than interpreting an unavailable PID/read lock as absence.

The standalone CLI explicitly selects fresh profile paths only for genuinely fresh or authoritatively retired lifetimes; IDE/default scope-derived paths remain unchanged. Opaque library launch errors/timeouts still retain an unobservable-attempt barrier and the actual old path, with no fallback, deletion, or replacement authorized. This does not prove retention of a child handle the dependency never returns, nor fix opaque launch-error fallback availability. Known pre-spawn preparation failures may retire without deleting an untrusted profile.

Five model regressions cover explicit profile policy, pre-spawn/opaque failure distinction, retained injected partial-cleanup authority and exact close retry, and canceled-waiter retirement visibility. A sixth deterministic regression drives production reservation/start/initialize with an opaque-launch error injected at the actual launch boundary; its close must remain residual, so removing the unobservable classification changes the oracle. The separately selected `real_chrome_partial_initialization_retains_child_and_retries_exact_close` test requires installed Chrome, isolated exact hosted `--ignored` selection, and `PHOENIX_TEST_CHROME_SHIM_EXECUTABLE` naming the matched actual CLI/server binary (never the unit-test executable). It asserts adopted group authority, injects initialization and cleanup failures around the actual returned Browser, checks exact child/aggregate retention and no second launch, retries authoritative group close, then permits a fresh CLI path only after retirement/reopen. The injected cleanup resource tests registry/retirement behavior, not native Browser/group teardown. Tests are authored but not locally executed; only standalone rustfmt/Allium validation is permitted locally. Exact hosted Rust, native group, and real-Chrome qualification of this candidate remain required. Earlier qualification receipts apply only to their exact earlier candidates.
Existing cleanup assertions are not weakened. No deletion retry loop is introduced; an explicit Close retry retains the exact stopped launch and rechecks profile ownership.

### Retention follow-up qualification boundary

The pre-spawn production driver regression `production_preparation_failure_retires_and_retries_the_same_ide_path` exercises failed preparation, retirement, repair, and retry through the same IDE-derived path. Fresh profile directory creation occurs after fallible executable/config preparation, so this failure does not create an unmarked profile that poisons the repaired scope. Pre-existing profiles remain untouched.

Typed `BrowserSessionLookup` distinguishes absent, ready, and retained initialization. The CLI maps retained initialization to `cleanup_failed` with its reservation lifetime, actual path, and failure reason; it does not report inactive absence. Real daemon regressions cover opaque launch failure and a returned-browser marker-write failure. These are authored, not locally executed.

The `dead_chrome_cleanup_failure_remains_visible_until_explicit_close` pre-Close diagnostic records the first cleanup-failure response and full owned process layout, then accepts only birth-identity disappearance or a kernel-exited/zombie observation. This observation is not cleanup authority. Strict resource absence after marker restoration and explicit Close is unchanged. The earlier hosted identity-disappearance timeout alone did not establish why the process identity remained; an unreaped zombie is a supported observation, not a measured cause until the diagnostic receipt shows it.

### Darwin exit-transition evidence

The original post-release failure was initially misattributed to adoption.
Exact-source line mapping corrected it to the exit-observation loop; the
readiness-only fixture change did not repair that failure. The immutable
`07302e161` and `e94a3887b` negatives retain this history.

Diagnostic source `8b4b2ff0b`, executed in
[run 39046690493](https://github.com/scottopell/phoenix-ide/actions/runs/39046690493),
returned a non-ready `waitid` while `proc_pidinfo` and `getpgid` returned ESRCH.
The immediately following sample, retaining the same unreaped child, returned
`SIGCHLD` / `CLD_EXITED` and the original PID from `waitid`. `SA_NOCLDWAIT` was not
set. The diagnostic test unconditionally failed after capture; it did not turn
that observation into success. Artifact `12106521163` retains the raw syscall
receipt.

The typed fix distinguishes `Live`, `ExitPending`, and `Exited`. On macOS only,
the observed unavailable-metadata/non-ready-wait combination is pending, not
live or exited. Pending authorizes neither group signalling nor drain completion;
the existing absolute teardown deadline bounds kernel observation. The exclusive
Tokio child remains retained, and wrong/reaped-child and birth-identity negatives
remain errors. Group drain completion still requires proven exit and no live
group members; this does not extend authority to escaped helpers.

Exact source `17a8f5952b742c192d61f437a61f5286863e24ac`, via qualifier
`7dfbfaa94c3e39f60f034649c8048499fcbd82a5`, passed both exact pure tests in
[macOS run 39047567616](https://github.com/scottopell/phoenix-ide/actions/runs/39047567616):
`exited_root_drains_stderr_closed_writer_before_profile_removal` (1 passed,
0 failed, 0.27 s) and `pending_exit_authorizes_neither_signal_nor_completion`
(1 passed, 0 failed). Job `117439269875`, artifact `12105519932`, retains both
zero-test-guarded receipts. This closes the narrow pure Darwin failure, **not**
actual Chrome, whole macOS, whole Linux, or final-head acceptance. Earlier Linux
native/full-Rust success belongs to the earlier `a236` source. The task-only main
advance `4894426888d175d4fd65095647bede65bb09718e` is composed without changing
production code; the composed source still owes joint whole-head qualification.
All diagnostic and negative tests are retained.

### Composed native and full-Rust qualification

Shared candidate `b2fd54fa83194d13bfa407cc61b23c16859c9024`, pinned to app
`aff5126e1e82cc72ef7a1bd567c57c0d3c487c4e` and main
`4894426888d175d4fd65095647bede65bb09718e`, completed all jobs.
[macOS run 39048480300](https://github.com/scottopell/phoenix-ide/actions/runs/39048480300)
passed native ownership/actual-Chrome qualification and the application build.
[Linux run 39048480354](https://github.com/scottopell/phoenix-ide/actions/runs/39048480354)
passed native ownership/actual-Chrome qualification, full Rust tests, E2E, tasks,
worker-budget, and qualification-plan checks. Rebuilt artifact `12107545384`
retains the six-report/six-export witness and successful normal owned drain.
These are actual runtime results, not inferred from the earlier pure tests.

That Linux run remained **failed overall** on two test-source issues: a missing
statement semicolon in the exit-observation test and a synchronous canonical-URL
assertion after an intermediate Coordinator render. Source children `8301cb58e`
and `4749e0d0d` respectively correct those issues without changing production
behavior, expected URL, lifecycle assertions, or timeouts. The UI failure did
not reproduce locally; the focused two cases and owning 18-test module passed
both before and after the awaited-URL correction. The successor source still
owes one fresh whole-head qualification. No full-acceptance, overhead-clearance,
escaped-helper coverage, or native-resume claim follows from these results.

### Darwin group-query contract and scoped verification

The macOS native failure on shared `35a10aeca` had no captured query return
values. The diagnostic-only `b8723941a` escaped-writer execution passed without
reproducing it; that negative remains unresolved as a specific causal trace.

The Apple XNU source audit, pinned to
[`f6217f891ac0bb64f3d375211650a4c1ff8ca1ea`](https://github.com/apple-oss-distributions/xnu/tree/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea),
found that `libsyscall/wrappers/libproc/libproc.c::proc_listpids` maps syscall
failure to zero while preserving errno, and successful kernel enumeration in
`bsd/kern/proc_info.c::proc_listpids` can return zero matching PID bytes.
Successful calls do not clear errno. The typed query wrapper therefore clears
and captures thread-local errno immediately around each call. The decoder accepts
zero only with zero errno and a valid capacity; errors, negative/non-PID-sized
counts, and full or over-capacity results remain fail-closed. Empty decoding does
not grant ownership: drain requires the exclusive retained child and proven exit
both before enumeration and after an empty observation.

[Run 39051330381](https://github.com/scottopell/phoenix-ide/actions/runs/39051330381)
executed exact `5bcfca51f` contract and escaped-writer tests successfully. Its raw
receipt showed four PID bytes/errno zero before root reap and zero bytes/errno
zero **after reap**. This proves successful empty enumeration only in that
post-reap observation, not zero results under retained-root authority, and does
not establish the earlier `35a10` failure's return values or cause. Post-reap
signal authority remains explicitly rejected.

Exact source `06c7512b1b8cbfd5ecc416cf606d27f502a6fb32`, via qualifier
`75dbbbeebe295d25748b4ebeb67bb8364691bcc8`, passed the complete seven-test native
module (0 failed, 0.29 s) and browser library/tests Clippy (2 min 14 s) in
[macOS run 39051966895](https://github.com/scottopell/phoenix-ide/actions/runs/39051966895).
Artifact `12107518333` retains the receipt. All Linux lanes on the older shared
`35a10` candidate also passed, including full Rust; its macOS native failure is
not erased or relabelled. The decoder successor still owes actual Chrome and
whole-head Linux/macOS qualification. No escaped-helper, overhead, native-resume,
or full-acceptance guarantee is added.

The source freshness check found main advanced from `489442688` to
`ea5df4b473ebd62ee8edd59daad9e43a119e9630` (published schema-114 launchd activation).
That deployment/harness/spec delta is composed in this source packet, with no
Rust, Cargo, or UI byte changes from the scoped `06c7512b1` proof. The published
activation ADR retains main's ID 115; the donor geometry ADR is remapped to 120.
Integration must refresh base/source/tree pins before whole-head qualification;
the scoped native proof is not relabelled as qualification of the merged main.

## Verification

Lifecycle and installed-command coverage live in `crates/phoenix-tools/tests/browser_cli.rs`. Browser-free preflight and operation-catalog parity live in `browser::access_tests`; profiling validation and methodology retain their existing unit coverage. Engine snapshot and console tests use actual Chrome for document replacement, native input, artifacts, source-event capture, and iframe diagnostics.

Local acceptance covers 26 CLI integration tests with actual Chrome on macOS (27 on Linux), four operation-boundary tests, 13 daemon failure/observation/image/client tests, nine console tests, the snapshot/native-input suite, and existing browser journeys. Ordinary console history survives navigation, including failed navigation; ordered document incarnations retain detached-frame errors while excluding late-created old-document frames. Inline images become verified existing artifacts or new private files, with visible conversion errors. Encoded and semantic request preflight runs before session lookup or offline dispatch. An isolated debug installation passed help/schema/open/eval/close smoke checks. Final CI and Codex review remain visible on the PR rather than being inferred from local checks.

ADR-110 records document attribution on the ordered stream, superseding ADR-109; ADR-111 extends capture to isolated iframe targets with bounded child-stream barriers. The hosted screenshot regression verifies that artifact metadata contains only the saved handle, while typed image bytes remain unchanged. Observed Chrome death retires the manager entry, transient profile, snapshot artifact, and profiling state before absence is reported. Partial request clients are bounded and isolated from the daemon's close, signal, and idle control loop. Scenario wait timeouts are validated before browser access. Failed retirement after observed Chrome death transfers its original error and browser identity into the retained stopping state; subsequent status remains truthful and only explicit shutdown retries cleanup.

Local broad validation passed Clippy, E2E, Allium, spec shape/anchors, task validation, and dev-tool tests. Its Rust run stopped on the unchanged database test `current_schema_controller_accepts_compiled_123_and_128_shapes`: the macOS Python SQLite controller reported `unable to open database file` during integrity checking; an isolated rerun reproduced it. Full-suite success is not claimed from that run.

The local database failure is tracked separately in task 41017.

Session identity tests distinguish non-UTF-8 Unix paths from each other and from literal replacement characters. A Linux executable regression verifies shared worktree identity across subdirectories and canonical symlink fallback; macOS filesystems reject the fixture's non-UTF-8 directory names, so byte encoding is tested there without filesystem creation.

Named session selection does not read the fallback working directory. Oversized saved scenario samples become verified artifacts; response encoding failures remain explicit and preserve nested artifact handles. Reference-click activation preserves scripted and native label forwarding across shadow roots, while typing re-enters focus enforcement before key dispatch.

Nested cross-site target teardown retires descendant sessions and their pending barriers together. Deterministic subtree ownership and native repeated A→B→A frame navigation regressions cover this cleanup boundary.

Open revalidates its snapshot document/generation after later observations and reports invalidation as a partial snapshot failure. Native-input paths share the guard cleanup lease; ordinary rejection awaits disarming. Heap-diff failure retains the completed post capture. Daemon readiness receives its own startup budget, and failed readiness terminates and reaps the unready child.

Typed evaluation uses one shared value producer and separate Phoenix-text/CLI-data renderers. CLI JSON retains native JSON values, distinguishes undefined and non-JSON scalars, and identifies JSON/text artifacts for outputs above 4 KiB. Evaluation data bypasses generic artifact extraction, preserving user object keys such as `path`, `post`, and `artifact_path`. Human output renders the authoritative value or artifact handle.

Screenshot output paths resolve in the caller's working directory before transport. Native browser coverage checks chosen/default paths, overwrite behavior, saved PNG bytes, and rejection of symlink or non-file destinations.

Repeated `open` reloads the current URL within the same browser lifetime, retaining cookies and origin storage while replacing page state. Fragment navigation follows native CDP semantics without depending on a document-load event for same-document completion. Real HTTP coverage checks document generations, fresh refs, navigation-local console errors, and server-loss reload failure followed by recovery in the same browser.

Evaluation exceptions retain one typed CDP diagnostic until rendering. Human output uses a bounded single-line message and source location; CLI structured errors retain the full exception context without interpreting user-thrown values as artifact metadata.

Command help includes a complete documentation URL in every build profile. Browser-free executable coverage invokes help outside the source checkout and checks that discovery does not create session state.
