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
extends ADR-116 without changing its detach-and-fence decision. REQ-SDC-015–021
specify separate same-build guardian and runner executables, owner-only Unix socket
IPC with bounded length-prefixed typed JSON, and explicit wire DTOs that cannot
materialize internal affine authority. The guardian mints one-use dispatch admissions
bound to the exact live instance, session, key, and full effect. The safe
`phoenix-deployment-controller` binary remains status/closeout only.

The disposable native transport uses separate guardian and runner executables over an
owner-only, same-UID, bounded protocol-v6 Unix socket whose Hello exchange requires the
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

Schema v6 is the exact schema of the standalone controller-owned disposable store. It is
not the Phoenix application database and is not a Phoenix application-database migration.
Exact predecessor schema-v5 controller stores and protocol-v5 peers fail closed before
mutation; no upgrade, downgrade, mixed-build, or guardian-replacement promise is made.

The disposable runner has structurally separate observe-only, initial-sample,
applied-ACK-recovery, activation-only `recover-and-finish`, bounded
`recover-finish-and-sample`, `recover-finish-and-child-list`, and
`recover-child-list-ack` modes. The initial runner creates
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
session after exit. In `recover-and-finish`, the ticket is retained but never
dispatched; dropping it leaves a consistent active checkpoint for disconnect recovery.
The protocol-v4 reconciliation units pass exact-head macOS qualification
([run 39008069477](https://github.com/scottopell/phoenix-ide/actions/runs/39008069477))
and full CI ([run 39008070976](https://github.com/scottopell/phoenix-ide/actions/runs/39008070976))
at `55a90279c54ea30c82f3803d80efd340db319a88`.

The new `recover-finish-and-sample` unit shares exactly that preflight/begin/completion/
prepare/durable/live activation path, retaining its original `PolicyLoop` and affine
sampling ticket. Only after both authorities are Active does it commit the exact issue,
dispatch once through native sampling, observe and atomically apply the retained completion,
and confirm ACK through the existing recovery helper. It requires exact-key
`Applied`/`AcknowledgeNow`, emits `recovered_sample_applied_ack` with attempt/revision/
ordinal/sequence, and exits without issuing the follow-up plan. A separate typed fixture
pauses at all three barriers, including `post_activation_sample_pending` before issue;
the third barrier cannot be selected in activation-only or other modes. The runner
parser regression `sample_continuation_and_third_barrier_are_structurally_mode_specific`
and the IPC regression
`native_sampling_admission_refuses_prepared_live_reconciling_even_after_durable_activation`
are authored. The latter checks admission refusal after preparation and with an exact
durable issue while live authority is still Reconciling, without submitting an effect.
The recovery plan is checked without issuing: the checkpoint's exact pending sequence must
match a singleton `SampleResources` plan before construction of the loop, and recovery calls
`issue_next` exactly once. Both initial and continuation dispatch use one helper. The client
returns a non-Clone opaque `UnsubmittedNativeSampling` capability only when execution is
known not to have occurred, closing the connection to fence any pre-submit admission.
Cancellation consumes that capability and transactionally deletes only its exact still-issued
row after checking ticket/effect/session/guardian identity and absence of application/ACK;
mismatch or storage failure rolls back and poisons the store. It returns no ticket. Ambiguous
post-submit transport/protocol outcomes retain the issue and return no cancellation custody.
Explicit post-submit refusals are cancellable only for the explicit pre-execution refusal
set; correlation failure, invalid-value and unexpected replies remain ambiguous. Authored
SQLite regressions cover exact deletion, rollback, absent-row replay, key/sequence/session/
guardian mismatch, and applied/confirmed refusal. Compile-fail examples cover construction
and Clone denial; client tests cover pre-admit refusal/connection closure and post-submit
ambiguity. An initial-runner regression covers deadline and admission-refusal cancellation.
Linux controller tests and all-target Clippy pass locally. Exact-head macOS
[run 39012241514](https://github.com/scottopell/phoenix-ide/actions/runs/39012241514)
and full CI [run 39012245789](https://github.com/scottopell/phoenix-ide/actions/runs/39012245789)
pass at `1d6f09bb40cf617a0fdbf13275e6297b8d047702`.

No schema, protocol, executor, general dispatch, or signal surface changes accompany this unit.

A distinct `recover-unapplied-sample` mode now covers runner death after the same live guardian
retains a successful first `SampleResources` result but before SQLite policy application. The
initial fixture's causal `sample_observed_pending_application` barrier proves one durable issue,
no application/confirmation, and exact retained custody before forced runner death. A fresh
runner commits interruption and a reconciling session, then combines an opaque store-issued
candidate with a dedicated live guardian response carrying exact issuing-session provenance.
A second causal `recovered_sample_verified_pending_application` barrier proves that killing this
replacement after verification still leaves the issue unapplied and guardian custody intact; a
third runner advances to a newer interruption/session before applying the same result once.
The resulting non-Clone application capability can only append a nonterminal sample to the
interrupted resource-sampling resume state and insert its exact application in one transaction.
Native time is read after the writer lock and immediately before commit. The operation returns
no ticket, runnable transition, activation, follow-up effect, or ACK authority; the existing
post-commit ACK path confirms the original result. Failed, terminal, missing, stale, mismatched,
ambiguous, and expired results remain retained and unapplied. Protocol v5 adds this dedicated
verification request and explicitly rejects v4; controller schema remains v5. Linux controller
tests, compile-fail doctests, and all-target Clippy pass locally. The direct and disposable
launchd death-before-application journeys and full controller matrix pass exact-head
qualification at `53c45538b98d25e90d749edf72402b001c7f5510`: macOS
[run 39022439278](https://github.com/scottopell/phoenix-ide/actions/runs/39022439278) and
full CI [run 39022440430](https://github.com/scottopell/phoenix-ide/actions/runs/39022440430).

A test-only affine `DisposableLaunchdGuardian` now generates a fresh owner-only
`/tmp/sdc-disposable-*` root and a `test.disposable.sdc.guardian.*` label. It bootstraps
only `gui/<current uid>` with a direct same-build guardian executable, no shell or
respawn, and private logs/plist/store/socket paths. Its bounded inspection correlates
the exact launchd job PID with fresh native UID/birth/parent/group observations.
Panic/explicit teardown attempts only original harmless-child cleanup and exact-label
bootout, confirms job absence separately from a loaded job without a PID, and retains
local evidence with an explicit warning if cleanup cannot be verified.

A separate two-job lifecycle journey stages same-build guardian and runner executables into
one private disposable root, then launches them as sibling jobs under internally generated
`test.disposable.sdc.guardian.*` and `test.disposable.sdc.runner.*` labels in the current
`gui/<uid>` domain. The launchd-owned runner pauses after atomic sample application and before
ACK; the fixture kills only its exact job label, archives that generation's private plist/logs,
and reinstalls the same runner label in typed ACK-recovery mode. The replacement remains
launchd-owned through durable confirmation, then exits after fixture release. The journey
requires exactly one issue/application/confirmation, unchanged guardian/child/deadline,
retained custody across runner death, no reconciliation completion/activation or follow-up,
and verified absence of both jobs at teardown. Staged executable bytes and digests, an inert
fake candidate artifact, and a separate marker-only fake application SQLite database are
checked before and after the journey. They are isolation witnesses only: no installer,
artifact activation, application migration, or production database operation occurs.
Exact-head macOS [run 39027451788](https://github.com/scottopell/phoenix-ide/actions/runs/39027451788)
and full CI [run 39027454259](https://github.com/scottopell/phoenix-ide/actions/runs/39027454259)
pass at `5c8526f561659afa206f5a406c05cffeb4107074`.

The preceding activation-only `native_launchd_process` three-runner journey passes
exact-head hosted launchd qualification
([run 39009502330](https://github.com/scottopell/phoenix-ide/actions/runs/39009502330)).
Full CI [run 39009503934](https://github.com/scottopell/phoenix-ide/actions/runs/39009503934)
also passed at `26692b6daef41ef2ae2c4ca80ae17eb87ed07350`. The protocol-v4 reconciliation
macOS/full-CI qualification recorded above remains separate evidence.

The separate `launchd_continues_one_sample_through_three_independent_runners` journey
selects `recover-finish-and-sample` for its final runner. The activation-only
`launchd_retains_original_parent_through_three_independent_runners` journey remains a
separate test using the shared launchd fixture, with exact one-issue/application/ACK
assertions and no retained-ticket dispatch. The continuation's three barriers assert
unchanged prior sampling rows; after the third release it requires exactly two settled
samples, dispatch at the recovery revision with next sequence 1, one reconciliation
completion/activation, no other retained result, runner-exit fencing, and exact-job teardown.
The added direct macOS unit
`fresh_runner_continues_exactly_one_retained_sample_after_two_phase_activation` checks the
same bounded continuation with a directly spawned guardian. Both correlate unchanged
guardian PID/birth/nonce, original child PID/birth/parent and absolute deadline. Existing
direct-process death/lost-ACK/activation-only journeys remain unchanged. Four launchd-free
shape/parser tests still cover disposable namespace/paths, direct escaped plist arguments,
exact-job absence classification, and complete native identity parsing.
The launchd-free continuation/parser/IPC units pass local Linux controller tests and all-target
Clippy. The direct and launchd process journeys pass exact-head macOS run `39012241514`.
Manual duplicate run `39012244386` failed explicitly before guardian startup when that runner
lacked GUI-domain bootstrap permission; it did not silently skip, fall back to a user/system
domain, or touch an installed agent.

Native owned-root enumeration advances the controller store and guardian protocol to
version 6; exact version-5 stores and peers are rejected without mutation or migration.
`recover-finish-and-child-list` retains the activated sampling `PolicyLoop`, applies the
second sample, consumes its actual `ListChildren` successor exactly once, commits an
exact owned-root issue, observes one bounded typed result, preserves received child order
in normalized result rows, applies the canonicalized PID set through that same loop, and
ACKs only after durable application. The harmless `sleep` root returns an empty result;
the no-crash runner finishes the ownership scan into `CleaningOwnedTree` and stops before
issuing `ObserveIdentity`. Empty enumeration is not cleanup or readiness evidence.

Distinct direct-process and two-job launchd journeys kill the exact runner after durable
child-list application and before ACK. A fresh same-live session rereads the exact
application, confirms the retained ACK, and proves one issue/application/confirmation,
zero result-child rows, no re-enumeration or reapplication, unchanged guardian/root
custody, and no signal or mutation. Fresh-session establishment interrupts ownership, so
these ACK-only recovery journeys correctly end `Interrupted`; identity reconciliation is
required before cleanup can resume. The direct no-crash journey
`fresh_runner_lists_guardian_root_children_without_mutating_custody` separately proves
`CleaningOwnedTree`.

The remaining native contract requires terminal/failed retained-sample settlement,
generalized mutation dispatch, a general continuing `PolicyLoop` over IPC, and
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
| REQ-SDC-011 | The standalone `ControllerSqliteStore` owns exact schema-v6 normalized attempt, policy-child, guardian-binding/session/slot, native sampling and child-list issue/application/ACK-confirmation, ordered child-result, and native reconciliation-completion/activation rows with revision compare-and-swap, strict load validation, affine recovery evidence, and fail-stop poisoning. It is distinct from the Phoenix application database. Production-exclusive store ownership remains a gap. |
| REQ-SDC-012 | `SupervisedChild` retains a direct child through original-group finalization. Native disconnect fences, fresh interruption revisions outpace unfinished reconciliation, and the original monotonic deadline refuses expired release/begin. The preceding disposable launchd ownership/activation-only journey passes run `39009502330`; the extended one-sample continuation passes run `39012241514`. Guardian replacement and production recovery remain unresolved risk. |
| REQ-SDC-013 | `driver` issues affine correlated tickets only from committed plans; exhaustive executor, CLI, completion-to-checkpoint, guardian integration, replay, and capacity-reservation coverage pass full CI/macOS at `907ca04670f46c5c2ea9f0b15993a996a83ac911`. |
| REQ-SDC-014 | Safe binary status/closeout, `open_existing`, typed JSON and category exits, and isolated CLI regressions cover missing stores, phase fences, CAS/load poisoning, malformed arguments, and non-native scope; hosted qualification passes at `907ca04670f46c5c2ea9f0b15993a996a83ac911`. |
| REQ-SDC-015 | Separate guardian and runner executables preserve the status/closeout-only safe CLI. The disposable initial runner owns one committed sampling round end to end; distinct replacement modes settle pending ACK, complete two-phase reconciliation, continue one retained sample, or recover one completed-but-unapplied nonterminal sample. The guardian remains the harmless child's original parent. Guardian-only launchd journeys pass runs `39009502330`, `39012241514`, and `39022439278`; a staged two-job guardian/runner lifecycle passes `39027451788`. No production installer or general continuing policy loop exists. |
| REQ-SDC-016 | Owner-only path checks, same-UID peer admission, exact source-input digest Hello identity, protocol-v6 four-byte big-endian bounded typed JSON framing, strict request fields, predecessor-v5/malformed/oversized-frame fencing, and replacement connections are implemented. Earlier protocol-v4/v5 qualification remains historical evidence; v6 exact-head qualification is recorded with REQ-SDC-021. |
| REQ-SDC-017 | Explicit wire DTOs remain separate from affine tickets. The native server independently validates durable evidence before consuming its initial gate, entering reconciliation, minting one-use exact sampling admission, verifying a retained issuing-session result for application-only recovery, dropping retained custody after application, or preparing and activating sampling reconciliation. Store evidence plus a correlated same-live response construct a distinct non-dispatch application capability; neither diagnostic source alone can construct it. Live preparation, durable activation, and final live activation each consume a distinct opaque capability; completion/activation rows and wire locators cannot construct them. General mutation remains refused. |
| REQ-SDC-018 | Normalized exact guardian/session/slot binding, binding-before-workload release, persisted liveness refresh, durable fresh interruption, and guardian-validated reconciliation are implemented for the bounded resource-sampling and owned-root child-list paths. Schema v6 persists exact sampling and child-list issue/application/confirmation, ordered child-result, immutable prepared completion, and separate atomic activation evidence, with relational insertion guards and rollback/replay tests. Separate fresh runners settle sampling or child-list `AppliedPendingAck` without redispatch or reapplication. Sampling activation-only recovery does not execute its retained ticket; a distinct continuation retains the original loop/ticket through activation and executes one new sample followed by its exact child-list successor. Terminal/failed retained settlement, identity reconciliation after child-list ACK recovery, and general continuing post-activation orchestration remain gaps. |
| REQ-SDC-019 | Exact controller-store schema compatibility, source-input build identity, protocol-v6 admission with explicit predecessor-v5 rejection, and platform capability checks exist. The transport remains pre-adoption and same-build-only; no schema-v5 upgrade, protocol-v5 replacement, downgrade, rolling, or cross-boot promise is made, and no Phoenix application-DB migration is involved. |
| REQ-SDC-020 | Linux policy/IPC/SQLite tests and macOS process tests cover exact source-input handshake, gated start, one native sample, original parenthood, runner death after application/before ACK, guardian receipt before a lost ACK response, same-live reconnection, durable confirmation with one issue/application, and pending-begin death/catch-up/two-phase activation without another effect; these pass exact-head macOS/full-CI qualification at `55a90279c54ea30c82f3803d80efd340db319a88`. The preceding test-only launchd three-runner application-death/ACK/reconciliation-activation journey and four launchd-free shape/parser units pass run `39009502330` and full CI `39009503934` at `26692b6daef41ef2ae2c4ca80ae17eb87ed07350`. The extended launchd and added direct-process three-barrier/one-sample-continuation journeys, mode-parser regression, and live-Reconciling admission regression pass macOS run `39012241514` and full CI `39012245789` at `1d6f09bb40cf617a0fdbf13275e6297b8d047702`. The new direct and launchd runner-death-before-application journeys, retained-provenance verifier, and interrupted nonterminal application regressions pass macOS run `39022439278` and full CI `39022440430` at `53c45538b98d25e90d749edf72402b001c7f5510`. The staged two-job launchd-owned runner lifecycle passes macOS run `39027451788` and full CI `39027454259` at `5c8526f561659afa206f5a406c05cffeb4107074`. Terminal/failed retained settlement, post-activation mutation, ownership-tree reconciliation, installer/helper integration, guardian replacement, updater, and rollback scenarios remain required. |
| REQ-SDC-021 | Schema-v6 child-list issue/application/ordered-result-child/ACK evidence, protocol-v6 exact owned-slot admission, bounded result decoding, same-loop application-before-ACK, and explicit predecessor-v5 rejection are implemented. `fresh_runner_lists_guardian_root_children_without_mutating_custody` proves empty enumeration reaches `CleaningOwnedTree` without `ObserveIdentity`; direct and two-job launchd application-before-ACK death journeys prove same-live exact ACK recovery without re-enumeration or reapplication and correctly remain `Interrupted` pending identity reconciliation. These paths pass hosted macOS run `39036754803` at implementation head `f72fcdb8349e318d274a041f6566a280b256ca28`. No arbitrary PID, signal, cleanup mutation, installer, migration, or production path is authorized. |

## Next work

The implemented test units are the in-process custodian/event loop and a disposable native
transport with durable initial binding/release, one exact sampling application, real runner
death after application/before ACK, a guardian-processed response lost before runner receipt,
same-live replacement-runner ACK confirmation, completed-before-application nonterminal
recovery into interrupted state, pending-begin death/catch-up, and later sampling reconciliation
completion/activation. Direct SQL tests reject FK-clean contradictory
checkpoint/binding/session evidence; failure tests reopen and compare durable rows, including
activation-insert rollback and unsettled-issue recovery refusal. Exact predecessor schema-v5 fixture coverage checks open/open_existing/read-only rejection
without schema or representative-row mutation. Controller-owned schema v6 retains normalized
sampling and child-list issue/application/confirmation rows, ordered child results, and
immutable reconciliation completion and activation records. The activation-only runner retains but does not dispatch its original
affine recovery ticket; the distinct continuation mode now carries that same ticket through
activation, one native application and ACK confirmation without issuing follow-up work.
The reconciliation units pass exact-head macOS/full-CI qualification at
`55a90279c54ea30c82f3803d80efd340db319a88`; the preceding launchd activation-only journey
passes `39009502330` and full CI `39009503934` at
`26692b6daef41ef2ae2c4ca80ae17eb87ed07350`.
The bounded continuation, mode-parser unit, live-Reconciling admission unit, direct-process
journey, and extended launchd journey pass exact-head macOS run `39012241514` and full CI
`39012245789` at `1d6f09bb40cf617a0fdbf13275e6297b8d047702`; no local launchctl invocation was
performed.
The completed-but-unapplied nonterminal recovery unit passes exact-head macOS run
`39022439278` and full CI `39022440430` at
`53c45538b98d25e90d749edf72402b001c7f5510`.
The staged two-job disposable lifecycle passes exact-head macOS run `39027451788` and full
CI `39027454259` at `5c8526f561659afa206f5a406c05cffeb4107074`. It demonstrates
private staging, launchd start, causal runner termination, same-label ACK recovery, and exact
stop/uninstall for fake resources; it does not demonstrate application deployment.

The remaining replacement gaps are explicit:

- The supported macOS installer remains `dev.py prod deploy --prepared-artifact ...
  --current-schema-upgrade` and its immutable manifest/retained Python helper. It has no safe
  disposable-root switch, and the Rust controller does not invoke or replace its signature,
  claim, ledger, application-DB, migration, publication, health, or forward-recovery authority.
- Launchd owns both disposable jobs only inside the test fixture. No installed controller
  selects initial versus recovery modes, exclusively owns the production controller store,
  supervises finite runners, or installs/removes production jobs.
- Native durable dispatch covers resource sampling and the exact guardian-owned root
  `ListChildren` successor. The no-crash path reaches `CleaningOwnedTree`; replacement ACK
  recovery remains `Interrupted` until exact identity reconciliation is implemented. No path
  signals an owned identity or reaches verified-clean/ready-for-disruption state.
- The policy core can retain exact candidate commit/digest and enter forward recovery, but the
  macOS adapter cannot reverify candidate bytes and no native runner invokes migration. There
  is no correlation yet with the supported helper's irreversible migration-start evidence.
- Guardian replacement, cross-boot adoption, downgrade, and mixed-build continuation remain
  unpromised. Adding them requires explicit compatibility and custody decisions rather than
  treating a matching label, PID, socket, or copied executable as the original guardian.

The next controller-owned work is terminal/failed retained-result settlement, exact child
identity acquisition/reconciliation after ACK-only recovery, general continuing orchestration,
and the remaining process-death matrix. Those surfaces must extend
REQ-SDC-015–021 and ADR-117/118/119 while preserving ADR-116 **detach-and-fence**, exact-instance
recovery, the original absolute deadline, retained results, monotonic reconciliation generations,
and whole-round signal-result reservation.

Installer and `dev.py` integration remain separately owned. The future boundary may
invoke only the supported prepared current-schema deploy seam; helper-owned schema,
ledger, claim, recovery, signature, and source-binding policy remain outside the Rust
controller. Independent guardian/runtime work does not wait on artifact identity, but
no helper invocation, native activation, deployment, or production adoption is
implemented or authorized by this qualification.
