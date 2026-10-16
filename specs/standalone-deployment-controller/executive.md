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
measurement, counter regression, and capacity arithmetic. Exact-head full CI
run `38805004723` and dedicated macOS run `38805004659` pass at commit
`907ca04670f46c5c2ea9f0b15993a996a83ac911`; no local heavy compilation was
performed for this source-only slice.

A controller-owned normalized SQLite store implements atomic checkpoint replacement,
durable compare-and-swap revisions, strict schema-version admission, affine crash-load
recovery, and fail-stop handle poisoning. Twenty SQLite integration tests and four
codec/schema unit tests cover relational round trips, corruption, stale writes,
recovery authority, and commit failure. The exact-head hosted runs above include
these tests. No production launchd label, process, application database, or
installation path is used by this crate.

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
that status/closeout surface adds no separate schema or dependency version.

The in-process guardian and deterministic `PolicyLoop` are hosted-qualified with
the dispatch, CLI, compile-fail authority, reconciliation-replay, and whole-signal-round
capacity regressions. Guardian result capacity is reserved for the complete mutation
round before its first ticket issues; retained historical evidence cannot restart an
unfinished reconciliation generation. This qualification does not establish process
survival, IPC correctness, or production orchestration.

## Native architecture contract

[ADR-109](../adrs/109_native-guardian-ipc-and-runner-are-same-build-pre-adoption.md)
extends ADR-108 without changing its detach-and-fence decision. REQ-SDC-015–020
specify separate same-build guardian and runner executables, owner-only Unix socket
IPC with bounded length-prefixed typed JSON, and explicit wire DTOs that cannot
materialize internal affine authority. The guardian mints one-use dispatch admissions
bound to the exact live instance, session, key, and full effect. The safe
`phoenix-deployment-controller` binary remains status/closeout only.

The disposable native transport uses separate guardian and runner executables over an
owner-only, same-UID, bounded protocol-v3 Unix socket whose Hello exchange requires the
same exact SHA-256 source-input build identity. Initial workload execution remains
blocked behind a process-local one-use gate until the runner commits a normalized exact
binding for a fresh resource-sampling checkpoint and the guardian independently rereads
the attempt, revision, nonce, guardian PID/birth identity, original nanosecond monotonic
deadline, active session, and ordered child PID/birth inventory. Persisted rows and wire
values cannot reconstruct that gate.

The guardian retains the original child across forced runner death and fences on EOF or
malformed frames. A replacement runner verifies the same live guardian and inventory,
durably fences unfinished sessions, refreshes crash-loaded interruption evidence to a
newer revision, and may enter observation-only `BeginReconciliation` after another pinned
store read. Changed slot liveness is persisted without replacing identity.
Native `SampleResources` dispatch consumes the runner's original affine ticket into
opaque application custody. Admission checks the exact issued-pending-application row,
active session and full guardian identity against one pinned store snapshot and live
guardian capacity/deadline before granting a guardian-local one-use admission; submission
rechecks the durable issue, consumes admission before execution, and retains the exact
typed completion for observation.
Successful applied samples may be acknowledged through the authenticated current active
or reconciling session. The guardian drops custody only after rereading exact application
evidence, retains a process-local correlated receipt, and returns that receipt idempotently
after a lost response on the same live guardian. The client turns only that correlated
response into opaque confirmation proof; confirmation is then committed as a normalized
row without replaying dispatch or policy application. Failed observations remain parked
and unacknowledgeable. Generic dispatch effects, signal/mutation, and reconciliation finish
remain refused.

Schema v4 is the exact schema of the standalone controller-owned disposable store. It is
not the Phoenix application database and is not a Phoenix application-database migration.
Existing schema-v3 controller stores fail closed; no v3-to-v4 upgrade, downgrade, mixed-build,
or guardian-replacement promise is made.

The disposable runner has structurally separate observe-only, initial-sample, and
applied-ACK-recovery modes. The initial runner creates and retains the committed transition
and affine ticket in-process through dispatch and application. A replacement runner discovers
only a typed pending-ACK obligation, reconnects to the same live guardian, begins a newer
observation-only reconciliation session, and confirms the ACK without redispatch or reapplication.
The remaining native contract requires reconciliation completion before new mutation,
generalized mutation dispatch, a continuing `PolicyLoop` over IPC, launchd ownership, and
production-exclusive store ownership. Guardian replacement, cross-boot recovery, installer
integration, and production adoption remain outside this pre-adoption qualification.

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
| REQ-SDC-011 | The standalone `ControllerSqliteStore` owns exact schema-v4 normalized attempt, policy-child, guardian-binding/session/slot, native sampling issue/application, and successful-sample ACK-confirmation rows with revision compare-and-swap, strict load validation, affine recovery evidence, and fail-stop poisoning. It is distinct from the Phoenix application database. Production-exclusive store ownership remains a gap. |
| REQ-SDC-012 | `SupervisedChild` retains a direct child through original-group finalization. Native disconnect fences, fresh interruption revisions outpace unfinished reconciliation, and the original monotonic deadline refuses expired release/begin. Guardian replacement, launchd ownership, and production recovery remain unresolved risk. |
| REQ-SDC-013 | `driver` issues affine correlated tickets only from committed plans; exhaustive executor, CLI, completion-to-checkpoint, guardian integration, replay, and capacity-reservation coverage pass full CI/macOS at `907ca04670f46c5c2ea9f0b15993a996a83ac911`. |
| REQ-SDC-014 | Safe binary status/closeout, `open_existing`, typed JSON and category exits, and isolated CLI regressions cover missing stores, phase fences, CAS/load poisoning, malformed arguments, and non-native scope; hosted qualification passes at `907ca04670f46c5c2ea9f0b15993a996a83ac911`. |
| REQ-SDC-015 | Separate guardian and runner executables preserve the status/closeout-only safe CLI. The disposable initial runner owns one committed sampling round end to end; a distinct replacement runner settles its pending ACK after death. The guardian remains the harmless child's original parent. Launchd ownership and a continuing runnable policy loop remain gaps. |
| REQ-SDC-016 | Owner-only path checks, same-UID peer admission, exact source-input digest Hello identity, protocol-v3 four-byte big-endian bounded typed JSON framing, strict request fields, version/malformed/oversized-frame fencing, and replacement connections pass Unix/macOS hosted tests. |
| REQ-SDC-017 | Explicit wire DTOs remain separate from affine tickets. The native server independently validates durable evidence before consuming its initial gate, entering reconciliation, minting one-use exact sampling admission, or dropping retained successful-sample custody after application; general mutation and reconciliation finish remain refused. |
| REQ-SDC-018 | Normalized exact guardian/session/slot binding, binding-before-workload release, persisted liveness refresh, durable fresh interruption, and guardian-validated observation-only reconciliation begin are implemented. Schema v4 persists exact sampling issue/application/confirmation evidence. The disposable runner executes the initial sampling round and a fresh process settles `AppliedPendingAck` in a newer reconciling session without redispatch or reapplication. Reconciliation completion remains a gap and is not required merely to release acknowledged custody. |
| REQ-SDC-019 | Exact controller-store schema compatibility, source-input build identity, protocol-v3 rejection, and platform capability checks exist. The transport remains pre-adoption and same-build-only; no controller-store v3 upgrade, downgrade, rolling/replacement/cross-boot promise is made, and no Phoenix application-DB migration is involved. |
| REQ-SDC-020 | Linux policy/IPC/SQLite tests and macOS process tests cover exact source-input handshake, gated start, one native sample, original parenthood, runner death after application/before ACK, guardian receipt before a lost ACK response, same-live reconnection, and durable confirmation with one issue/application. Runner death before application commit, reconciliation completion/new mutation, guardian replacement, launchd, updater, and rollback scenarios remain required. |

## Next work

The implemented test units are the in-process custodian/event loop and a disposable native
transport with durable initial binding/release, one exact sampling application, real runner
death after application/before ACK, a guardian-processed response lost before runner receipt,
and same-live replacement-runner ACK confirmation during observation-only reconciliation.
Controller-owned schema v4 retains one issue/application/confirmation across both journeys.
The next controller-owned work is runner death before application commit, native reconciliation
completion before any new mutation, then a continuing policy loop over the IPC boundary,
launchd ownership, and the remaining process-death matrix. Those surfaces must complete
REQ-SDC-015–020 and ADR-109 while preserving ADR-108 **detach-and-fence**, exact-instance
recovery, the original absolute deadline, retained results, monotonic reconciliation generations,
and whole-round signal-result reservation.

Installer and `dev.py` integration remain separately owned. The future boundary may
invoke only the supported prepared current-schema deploy seam; helper-owned schema,
ledger, claim, recovery, signature, and source-binding policy remain outside the Rust
controller. Independent guardian/runtime work does not wait on artifact identity, but
no helper invocation, native activation, deployment, or production adoption is
implemented or authorized by this qualification.
