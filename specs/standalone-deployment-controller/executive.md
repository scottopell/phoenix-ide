# Standalone Deployment Controller — Executive Status

## Current reality

A Rust policy crate models deployment phases, resource admission, stable process
identity, owned-tree evidence, bounded cleanup, interruption, administrative
closeout, and the post-migration forward-recovery fence. A target-specific macOS
adapter supplies native process observation, resource sampling, and retained
direct-child supervision in an isolated process group. The general observation
adapter intentionally cannot signal arbitrary observed PIDs. The crate has no
production caller, and existing deployment behavior remains unchanged.

The deterministic suite contains 22 fake-OS tests covering the observed
child-ESRCH-before-birth race, PID reuse, reparenting, exact-identity signaling,
permission uncertainty, absolute-deadline closeout, repeated interruption,
complete recovery reconciliation, expired resource-sampling recovery, sustained
contention, and exact-candidate forward recovery. Five harmless macOS integration
tests plus five adapter unit regressions cover supervised-child finalization,
bounded best-effort teardown, arbitrary-PID mutation rejection, native resource
measurement, counter regression, and capacity arithmetic. Exact-head hosted Rust,
Clippy, and native macOS qualification is pending because the local host did not
satisfy its compiler admission threshold.

A controller-owned normalized SQLite store implements atomic checkpoint replacement,
durable compare-and-swap revisions, strict schema-version admission, affine crash-load
recovery, and fail-stop handle poisoning. Twenty SQLite integration tests and four
codec/schema unit tests cover relational round trips, corruption, stale writes,
recovery authority, and commit failure. Hosted exact-head qualification remains
pending. No production launchd label, process, reservation, application database, or
installation path is used by this crate.

## Requirement coverage

| Requirement | Current coverage / gap |
| --- | --- |
| REQ-SDC-001 | `Transition` separates the durable checkpoint from `after_persist` OS effects; `ControllerSqliteStore` commits the normalized checkpoint before returning `CommittedTransition`. |
| REQ-SDC-002 | `ResourceWindow` uses ordered capacity/contention samples with sustained-contention regression coverage and contains no process metadata. |
| REQ-SDC-003 | `ProcessIdentity`, `BirthToken`, `DiscoveryId`, and `CandidateDiscovery` separate stable ownership from PID hints. |
| REQ-SDC-004 | Typed observation failures remain unresolved through the deadline; the 5188/5189 fake-OS regression covers transient ESRCH before birth acquisition. |
| REQ-SDC-005 | `OwnedProcessTree` and the fake OS recheck exact identity before signaling; PID reuse and reparenting regressions are present. |
| REQ-SDC-006 | `CleanupReadiness::VerifiedClean` requires all exact identities absent and no unresolved discovery. |
| REQ-SDC-007 | Attempt-bound `AdministrativeRiskAcceptance` closes without a cleanup claim or OS effect and retains evidence. |
| REQ-SDC-008 | Recovery marks owned identities for fresh observation and preserves the original deadline before exposing signal effects. |
| REQ-SDC-009 | `CompatibilityBoundary::CandidateMigrationInvoked` retains full commit plus artifact digest and recovers only through candidate reverification. |
| REQ-SDC-010 | `ControllerOs` is typed and fake-backed; the macOS adapter implements native observation and fail-closed capacity/contention sampling, including counter-regression and capacity-arithmetic regressions. Its harmless native qualification is pending exact-head hosted results. |
| REQ-SDC-011 | `ControllerSqliteStore` owns an independent schema with normalized child rows, full-transaction checkpoint replacement, revision compare-and-swap, strict load validation, affine recovery evidence, and fail-stop poisoning. Exact-head hosted tests and Clippy are pending. |
| REQ-SDC-012 | `SupervisedChild` retains a direct child as an isolated process-group leader until original-group finalization. Leader exit alone cannot finalize while same-group descendants remain; Drop is bounded best-effort and cannot claim cleanup. Arbitrary observed PID signaling is unsupported; group escape, deliberate group join, and controller-exit risks remain explicit. Exact-head hosted macOS qualification is pending. |

## Next work

Qualify the exact source commit through hosted Rust, Clippy, spec, and harmless
macOS-native checks, then reconcile an independent adversarial review of the durable
store and adapter. Thin `dev.py` delegation remains serialized with the installer and
host-reconciliation owners and requires a fresh overlap receipt; production adoption
is outside this slice.
