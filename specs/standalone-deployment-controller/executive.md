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
measurement, counter regression, and capacity arithmetic. Hosted Rust, Clippy, and
native macOS qualification passed for the implementation tree at commit
`9518d50ef75ee9e804469c3417c3fc1960ff5c8a`; local compilation was not attempted
because the host did not satisfy its compiler admission threshold.

A controller-owned normalized SQLite store implements atomic checkpoint replacement,
durable compare-and-swap revisions, strict schema-version admission, affine crash-load
recovery, and fail-stop handle poisoning. Twenty SQLite integration tests and four
codec/schema unit tests cover relational round trips, corruption, stale writes,
recovery authority, and commit failure. Full CI run `38752367219` and dedicated
macOS run `38752367467` passed. No production launchd label, process, reservation,
application database, or installation path is used by this crate.

The crate also contains an affine dispatch boundary: consuming a non-cloneable
`CommittedTransition` mints one batch with private, non-cloneable tickets keyed by
attempt, committed revision, and zero-based ordinal. `EffectExecutor` maps each of
the six effects to one typed `ControllerOs` call and correlated completion without
policy, retries, or activation authority. Diagnostic effect values cannot mint
tickets; dropped tickets do nothing. This is not exactly-once execution across crash.

The `phoenix-deployment-controller` binary exposes only safe `status` and explicit
administrative `closeout` against an existing caller-selected controller store.
`open_existing` omits SQLite's create flag and does not initialize schemas. Status
reads affine evidence into a four-field JSON summary, never a live checkpoint.
Closeout phase-checks before recovery preparation, persists matching risk acceptance
through CAS, and requires an empty dispatch batch. Fixed error categories and exit
codes avoid raw SQL, path, command, or environment disclosure. `serde`/`serde_json`
serialize the typed summary and phase/outcome enums instead of hand-assembling JSON;
there is no controller schema change or new dependency version.

The dispatch and CLI regression suite and compile-fail authority examples are
authored but have not been compiled or executed in this slice because host admission
prohibits local Rust build/test/Clippy. Static qualification is limited to formatting,
Cargo metadata/lock resolution, diff checks, and Allium; hosted Rust and CLI
qualification remains required before claiming runtime acceptance of this slice.

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
| REQ-SDC-010 | `ControllerOs` is typed and fake-backed; hosted macOS tests qualify native observation and fail-closed capacity/contention sampling, including counter-regression and capacity-arithmetic regressions. |
| REQ-SDC-011 | `ControllerSqliteStore` owns an independent schema with normalized child rows, full-transaction checkpoint replacement, revision compare-and-swap, strict load validation, affine recovery evidence, and fail-stop poisoning; hosted tests and Clippy pass. |
| REQ-SDC-012 | `SupervisedChild` retains a direct child through original-group finalization. The in-process `Guardian`/`PolicyLoop` author detach-and-fence custody, exact instance/session binding, retained-result acknowledgement, reconciliation revision floors, and fail-stop integration. Harmless native supervision passes; launchd IPC and controller-death qualification remain unimplemented. |
| REQ-SDC-013 | `driver` issues affine correlated tickets only from committed plans; exhaustive executor and CLI coverage passed full CI/macOS at `802123f34813c601f9830b63e373d43491d0823d`. The new completion-to-checkpoint and guardian integration tests are authored and await hosted qualification. |
| REQ-SDC-014 | Safe binary status/closeout, `open_existing`, typed JSON and category exits, and isolated CLI regressions cover missing stores, phase fences, CAS/load poisoning, malformed arguments, and non-native scope; hosted qualification passes at `802123f34813c601f9830b63e373d43491d0823d`. |

## Next work

A deterministic `PolicyLoop` and bounded in-process `Guardian` now model completion
persistence, retained-result acknowledgement, session fencing, detach-and-fence
recovery, reconciliation revision floors, and oversized mutation fail-closed behavior.
Their deterministic tests and privacy examples are authored but not locally compiled
or executed under source-only admission; hosted compilation, tests, doctests, and
Clippy must qualify this new slice. No launchd IPC, native guardian process, deploy,
installer, activation, or `run` command is exposed, so this is not production
crash-survival capability.

PR 919 released the qualified installer seam at merge
`0304adc6841f5fb8297d9bc036d891309083a6e3`: `dev.py` remains blob
`c2e9b44b11350f859b0cf168aad4612fb3bc6f06`, and the launchd helper is blob
`9a2ba2949c9f12ce7e8fa404e8b29fca037d5af2`. The supported future boundary is the
prepared current-schema `dev.py prod deploy` command; helper-owned schema, ledger,
claim, recovery, signature, and source-binding policy remains outside the controller.
This slice does not invoke that boundary. Controller loss uses the ADR-108
**detach-and-fence** policy; the in-process protocol model is authored, while launchd
transport/native custody and disposable process-death/reconnect qualification remain
unimplemented. Production adoption remains blocked.
