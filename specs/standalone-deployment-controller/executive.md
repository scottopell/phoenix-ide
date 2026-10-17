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

[ADR-117](../adrs/117_native-guardian-ipc-and-runner-are-same-build-pre-adoption.md)
extends ADR-116 without changing its detach-and-fence decision. REQ-SDC-015–020
specify separate same-build guardian and runner executables, owner-only Unix socket
IPC with bounded length-prefixed typed JSON, and explicit wire DTOs that cannot
materialize internal affine authority. The guardian mints one-use dispatch admissions
bound to the exact live instance, session, key, and full effect. The safe
`phoenix-deployment-controller` binary remains status/closeout only.

The disposable native transport uses separate guardian and runner executables over an
owner-only, same-UID, bounded protocol-v4 Unix socket whose Hello exchange requires the
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
and unacknowledgeable. A separate sampling-recovery runner can complete the fresh
resource-sampling reconciliation barrier without dispatching its retained recovery ticket.
The implemented recovery unit atomically commits immutable prepared completion evidence
with the recovery checkpoint. A crate-private evidence reader lets the same live guardian
correlate that evidence and consume the private reconciliation permit into a staged, non-active
capability. The runner must consume the corresponding opaque client capability while atomically
recording a separate normalized activation, advancing the binding revision and dispatch floor,
and making the exact session durably Active. A final locator-only activation request causes the
guardian to reread that durable activation and consume its staged capability before becoming
live `Active`. Crashes before durable activation leave both authorities reconciling; crashes
afterward leave the guardian non-active until activation or disconnect fencing. Completion and
activation diagnostics have private fields and cannot reconstruct either capability. Generic
dispatch effects and signal/mutation remain refused.

Schema v5 is the exact schema of the standalone controller-owned disposable store. It is
not the Phoenix application database and is not a Phoenix application-database migration.
Existing schema-v3 and schema-v4 controller stores fail closed; no upgrade, downgrade,
mixed-build, or guardian-replacement promise is made.

The disposable runner has structurally separate observe-only, initial-sample,
applied-ACK-recovery, and sampling-reconciliation-completion modes. The initial runner creates
and retains the committed transition and affine ticket in-process through dispatch and
application. One replacement runner discovers only a typed pending-ACK obligation, reconnects
to the same live guardian, begins a newer reconciliation session, and confirms the ACK without
redispatch or reapplication. The recovery test unit uses the exact runner preflight to
check fresh native monotonic time against the stored deadline before binding verification
or session/checkpoint mutation. Its bounded recover-and-finish-only fixture pauses after
durable fresh binding and before live begin. The macOS process test kills that runner,
checks the abandoned pending generation, then starts a new runner whose catch-up fences
that generation before advancing to a strictly newer one. The activation-barrier fixture additionally proves live `Reconciling` after preparation and
after durable activation, then live `Active` only after the final request. The test asserts
unchanged sampling counts and guardian/child/deadline, and detach from the durably Active
session after exit. The ticket is retained but never
dispatched; dropping it leaves a consistent active checkpoint for disconnect recovery.
The protocol-v4 reconciliation units pass exact-head macOS qualification
([run 39008069477](https://github.com/scottopell/phoenix-ide/actions/runs/39008069477))
and full CI ([run 39008070976](https://github.com/scottopell/phoenix-ide/actions/runs/39008070976))
at `55a90279c54ea30c82f3803d80efd340db319a88`.

A test-only affine `DisposableLaunchdGuardian` now generates a fresh owner-only
`/tmp/sdc-disposable-*` root and a `test.disposable.sdc.guardian.*` label. It bootstraps
only `gui/<current uid>` with a direct same-build guardian executable, no shell or
respawn, and private logs/plist/store/socket paths. Its bounded inspection correlates
the exact launchd job PID with fresh native UID/birth/parent/group observations.
Panic/explicit teardown attempts only original harmless-child cleanup and exact-label
bootout, confirms job absence separately from a loaded job without a PID, and retains
local evidence with an explicit warning if cleanup cannot be verified.

The authored `native_launchd_process` integration target contains one three-runner
journey: initial sampling application then forced runner death, fresh ACK settlement,
and fresh protocol-v4 reconciliation preparation/durable activation/live activation.
Barriers check same guardian PID/birth/nonce, original child PID/birth/parent, original
deadline, one issue/application/confirmation and one completion/activation, then
runner-exit fencing and exact-job teardown. Four launchd-free shape/parser tests cover
the disposable namespace/paths, direct escaped plist arguments, exact-job absence
classification, and complete native identity parsing. Existing direct-process tests
retain their journeys and share only extracted sampling command/count helpers.
These units are authored, not compiled or execution-qualified by this slice; local
validation is rustfmt and diff whitespace checking only. Hosted macOS qualification
requires the invoking UID's logged-in GUI domain. Missing domain/bootstrap permission
is an explicit failure, never a silent skip or user/system-domain fallback.

The remaining native contract requires hosted launchd qualification, generalized
mutation dispatch, a continuing `PolicyLoop` over IPC, and production-exclusive store
ownership. Guardian replacement, cross-boot recovery, installer integration, and
production adoption remain outside this pre-adoption qualification.

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
| REQ-SDC-011 | The standalone `ControllerSqliteStore` owns exact schema-v5 normalized attempt, policy-child, guardian-binding/session/slot, native sampling issue/application/ACK-confirmation, and native reconciliation-completion/activation rows with revision compare-and-swap, strict load validation, affine recovery evidence, and fail-stop poisoning. It is distinct from the Phoenix application database. Production-exclusive store ownership remains a gap. |
| REQ-SDC-012 | `SupervisedChild` retains a direct child through original-group finalization. Native disconnect fences, fresh interruption revisions outpace unfinished reconciliation, and the original monotonic deadline refuses expired release/begin. The disposable launchd ownership test unit is authored but not hosted-qualified; guardian replacement and production recovery remain unresolved risk. |
| REQ-SDC-013 | `driver` issues affine correlated tickets only from committed plans; exhaustive executor, CLI, completion-to-checkpoint, guardian integration, replay, and capacity-reservation coverage pass full CI/macOS at `907ca04670f46c5c2ea9f0b15993a996a83ac911`. |
| REQ-SDC-014 | Safe binary status/closeout, `open_existing`, typed JSON and category exits, and isolated CLI regressions cover missing stores, phase fences, CAS/load poisoning, malformed arguments, and non-native scope; hosted qualification passes at `907ca04670f46c5c2ea9f0b15993a996a83ac911`. |
| REQ-SDC-015 | Separate guardian and runner executables preserve the status/closeout-only safe CLI. The disposable initial runner owns one committed sampling round end to end; a distinct replacement runner settles its pending ACK after death. The guardian remains the harmless child's original parent. A test-only exact-label launchd adapter and three-independent-runner death/ACK/two-phase-activation journey are authored but not hosted-qualified. A continuing runnable policy loop remains a gap. |
| REQ-SDC-016 | Owner-only path checks, same-UID peer admission, exact source-input digest Hello identity, protocol-v4 four-byte big-endian bounded typed JSON framing, strict request fields, version-3/malformed/oversized-frame fencing, and replacement connections pass exact-head macOS/full-CI qualification at `55a90279c54ea30c82f3803d80efd340db319a88`. |
| REQ-SDC-017 | Explicit wire DTOs remain separate from affine tickets. The native server independently validates durable evidence before consuming its initial gate, entering reconciliation, minting one-use exact sampling admission, dropping retained successful-sample custody after application, or preparing and activating sampling reconciliation. Live preparation, durable activation, and final live activation each consume a distinct opaque capability; completion/activation rows and wire locators cannot construct them. General mutation remains refused. |
| REQ-SDC-018 | Normalized exact guardian/session/slot binding, binding-before-workload release, persisted liveness refresh, durable fresh interruption, and guardian-validated reconciliation are implemented for the bounded resource-sampling path. Schema v5 persists exact sampling issue/application/confirmation, immutable prepared completion, and separate atomic activation evidence, with relational insertion guards and rollback/replay tests. Separate fresh runners settle `AppliedPendingAck` and later complete a newer reconciliation generation without redispatch, reapplication, or executing the retained recovery ticket. Ownership-tree reconciliation and a continuing post-activation policy loop remain gaps. |
| REQ-SDC-019 | Exact controller-store schema compatibility, source-input build identity, protocol-v4 admission with explicit version-3 rejection, and platform capability checks exist. The transport remains pre-adoption and same-build-only; no controller-store v3/v4 upgrade, downgrade, rolling/replacement/cross-boot promise is made, and no Phoenix application-DB migration is involved. |
| REQ-SDC-020 | Linux policy/IPC/SQLite tests and macOS process tests cover exact source-input handshake, gated start, one native sample, original parenthood, runner death after application/before ACK, guardian receipt before a lost ACK response, same-live reconnection, durable confirmation with one issue/application, and pending-begin death/catch-up/two-phase activation without another effect; these pass exact-head macOS/full-CI qualification at `55a90279c54ea30c82f3803d80efd340db319a88`. A separate test-only launchd target adds the three-runner application-death/ACK/reconciliation-activation journey and four launchd-free shape/parser regressions; launchd remains unqualified. Runner death before application commit, post-activation mutation, ownership-tree reconciliation, guardian replacement, updater, and rollback scenarios remain required. |

## Next work

The implemented test units are the in-process custodian/event loop and a disposable native
transport with durable initial binding/release, one exact sampling application, real runner
death after application/before ACK, a guardian-processed response lost before runner receipt,
same-live replacement-runner ACK confirmation, pending-begin death/catch-up, and later sampling
reconciliation completion/activation. Direct SQL tests reject FK-clean contradictory
checkpoint/binding/session evidence; failure tests reopen and compare durable rows, including
activation-insert rollback and unsettled-issue recovery refusal. Exact HEAD schema-v4 fixture
coverage checks open/open_existing rejection without schema or representative-row mutation.
Controller-owned schema v5 retains one issue/application/confirmation plus immutable completion
and activation records while the finishing runner retains but does not dispatch its original
affine recovery ticket. The reconciliation units pass exact-head macOS/full-CI qualification
at `55a90279c54ea30c82f3803d80efd340db319a88`.
The disposable launchd ownership/three-runner test unit awaits compilation
and hosted GUI-domain qualification; no launchctl or Cargo invocation was performed locally.
The next controller-owned work is runner death before application commit, a continuing policy
loop that can carry the retained post-reconciliation plan across the IPC boundary,
ownership-tree reconciliation, and the remaining process-death matrix. Those surfaces must complete
REQ-SDC-015–020 and ADR-117 while preserving ADR-116 **detach-and-fence**, exact-instance
recovery, the original absolute deadline, retained results, monotonic reconciliation generations,
and whole-round signal-result reservation.

Installer and `dev.py` integration remain separately owned. The future boundary may
invoke only the supported prepared current-schema deploy seam; helper-owned schema,
ledger, claim, recovery, signature, and source-binding policy remain outside the Rust
controller. Independent guardian/runtime work does not wait on artifact identity, but
no helper invocation, native activation, deployment, or production adoption is
implemented or authorized by this qualification.
