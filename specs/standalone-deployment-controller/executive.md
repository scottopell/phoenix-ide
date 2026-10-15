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
there is no controller schema change or new dependency version.

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

A disposable native transport is implemented and hosted-qualified at commit
`3481095630f86bc7e88828949a109918af3cc12f`: full CI run `38820157087` and macOS
run `38820157097` pass. Separate guardian and runner executables use an owner-only,
same-UID, bounded framed Unix socket. The guardian originally parents a harmless child,
retains it across forced runner death, fences on EOF/malformed frames, and accepts a
replacement observer only against the same live instance and inventory. Reconciliation
and mutation remain typed `UnsupportedDurableBinding` refusals rather than fabricated
authority.

The remaining native contract requires normalized guardian/session/inventory/result
binding, durable policy application before ACK, fresh interruption/reconciliation within
the original deadline, and actual launchd ownership. Schema-v1 upgrades, downgrade,
rolling versions, guardian replacement, cross-boot recovery, installer integration, and
production adoption remain explicitly outside this pre-adoption qualification.

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
| REQ-SDC-011 | `ControllerSqliteStore` owns an independent schema with normalized policy child rows, full-transaction checkpoint replacement, revision compare-and-swap, strict load validation, affine recovery evidence, and fail-stop poisoning; hosted tests and Clippy pass. Native guardian/session/inventory/result rows required by REQ-SDC-018 remain an implementation/qualification gap. |
| REQ-SDC-012 | `SupervisedChild` retains a direct child through original-group finalization. The hosted-qualified in-process `Guardian`/`PolicyLoop` enforce detach-and-fence custody, exact instance/session binding, retained-result acknowledgement, strictly increasing reconciliation generations, and whole-signal-round capacity reservation. A disposable native guardian now remains the original parent across forced runner death; launchd ownership and durable policy recovery remain gaps. |
| REQ-SDC-013 | `driver` issues affine correlated tickets only from committed plans; exhaustive executor, CLI, completion-to-checkpoint, guardian integration, replay, and capacity-reservation coverage pass full CI/macOS at `907ca04670f46c5c2ea9f0b15993a996a83ac911`. |
| REQ-SDC-014 | Safe binary status/closeout, `open_existing`, typed JSON and category exits, and isolated CLI regressions cover missing stores, phase fences, CAS/load poisoning, malformed arguments, and non-native scope; hosted qualification passes at `907ca04670f46c5c2ea9f0b15993a996a83ac911`. |
| REQ-SDC-015 | Separate guardian and runner executables preserve the status/closeout-only safe CLI. The guardian is the harmless child's original parent and survives forced runner death; launchd ownership and a durable runnable policy loop remain gaps. |
| REQ-SDC-016 | Owner-only path checks, same-UID peer admission, four-byte big-endian bounded typed JSON framing, malformed/oversized-frame fencing, and replacement connections pass Unix/macOS hosted tests. Exact-build artifact identity is not yet carried in the handshake. |
| REQ-SDC-017 | Explicit wire DTOs remain separate from affine tickets. The in-memory server qualifies one-use staged admissions, but cross-process mutation remains refused until durable binding exists. |
| REQ-SDC-018 | A replacement observer proves the same live instance and inventory after runner death. Normalized native binding, durable-before-ACK custody, fresh interruption/reconciliation, and ACK crash windows remain unimplemented. |
| REQ-SDC-019 | The transport is pre-adoption and same-build-only by construction; no v1 upgrade, downgrade, rolling/replacement/cross-boot promise is made. Explicit artifact-version handshake rejection remains a gap. |
| REQ-SDC-020 | A real disposable guardian/runner/harmless-child test proves original parenthood, runner death, surviving child/guardian identity, EOF fencing, and observation-only reconnect. Launchd bootstrap, SQLite recovery, mutation, and ACK-window cases remain required. |

## Next work

The qualified units are the in-process custodian/event loop and a disposable native
observation transport with real runner-death survival. The next controller-owned work is
a normalized durable binding, a runnable policy loop over the IPC boundary, launchd
ownership, and the remaining process-death/reconnect/ACK-window E2E matrix. Those
surfaces must complete REQ-SDC-015–020 and ADR-109 while preserving ADR-108
**detach-and-fence**, exact-instance recovery, the original absolute deadline, retained
results, monotonic reconciliation generations, and whole-round signal-result
reservation. Qualification must include runner death before application commit,
after commit/before ACK, lost ACK, and refusal of stale or mismatched authority.

Installer and `dev.py` integration remain separately owned. The future boundary may
invoke only the supported prepared current-schema deploy seam; helper-owned schema,
ledger, claim, recovery, signature, and source-binding policy remain outside the Rust
controller. Independent guardian/runtime work does not wait on artifact identity, but
no helper invocation, native activation, deployment, or production adoption is
implemented or authorized by this qualification.
