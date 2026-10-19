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
`907ca04670f46c5c2ea9f0b15993a996a83ac911`.

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

[ADR-132](../adrs/132_native-guardian-ipc-and-runner-are-same-build-pre-adoption.md)
extends ADR-131 without changing its detach-and-fence decision. REQ-SDC-015–022
specify separate same-build guardian and runner executables, owner-only Unix socket
IPC with bounded length-prefixed typed JSON, and explicit wire DTOs that cannot
materialize internal affine authority. The guardian mints one-use dispatch admissions
bound to the exact live instance, session, key, and full effect. The safe
`phoenix-deployment-controller` binary remains status/closeout only.

The disposable native transport uses separate guardian and runner executables over an
owner-only, same-UID, bounded protocol-v10 Unix socket whose Hello exchange requires the
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

Schema v8 is the exact schema of the standalone controller-owned disposable store. It is
not the Phoenix application database and is not a Phoenix application-database migration.
Exact predecessor schema-v7 controller stores and protocol-v9 peers fail closed before
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

The controller uses schema v8 and same-build guardian protocol v10; exact schema-v7
stores and protocol-v9 peers are rejected without mutation or migration. Schema v8 retains
the normalized exact-root evidence and adds immutable native sampling failure settlement
and distinct settlement-ACK rows. Protocol v10 adds key/session/provenance-only failed-result
verification and ACK: the guardian derives the exact typed failure from durable evidence and
retained custody. Generic native dispatch still admits sampling only.

Child enumeration now covers both crash windows. Application-before-ACK recovery rereads
the exact durable application. Completed-before-application recovery combines the sole
pending issue with same-live retained issuing provenance to create an application-only
capability; it reconstructs no ticket, performs no second enumeration, applies once while
remaining `Interrupted`, and then uses the existing ACK gate. Direct-process and
same-label two-job launchd journeys prove one issue/application/confirmation, preserved
ordered child evidence, unchanged guardian/root/deadline custody, and no successor effect.

The no-crash bounded continuation closes the empty root scan, waits for the harmless root
to exit naturally, and consumes the exact policy-issued `ObserveIdentity` obligation. The
guardian reaps only when the original process group is empty; a retained group member
produces unresolved evidence. Exact absence is atomically applied through the same
`PolicyLoop`, persisted before ACK, and then policy waits for the unchanged deadline and
enters `ReadyForDisruption` with `VerifiedClean` and zero `SignalExactIdentity` effects.
Direct-process and same-label two-job launchd journeys cover this no-signal route. A
separate direct-process death-after-identity-application journey proves ACK-only recovery
without re-observation and correctly stops `Interrupted`.

Completed-before-application identity recovery uses the same schema-v7 rows. A fresh
same-live reconciling session combines the sole pending issue with exact retained issuing
provenance and unchanged root slot to obtain application-only authority. The guardian
request carries no PID or birth token and performs no second observation. The transaction
checks the original deadline after acquiring writer ownership and immediately before
commit, atomically applies the retained typed result, remains `Interrupted`, emits no
successor effects, and then uses the existing durable-application ACK path. Direct-process
and same-label launchd journeys pass exact-head macOS run `39071653753` and full CI run
`39071653757` at `d66f5a5847d8add572fd742c68d25bfef3fde3d1`.

Already-durable `ParkedRetained` sampling failures have a separate custody-settlement
lifecycle. A fresh same-live reconciling session verifies the sole exact failure and original
provenance, atomically advances only the interrupted checkpoint revision and settlement row
under the original deadline, then independently ACKs and confirms custody release. Settlement
never satisfies the successful-sample barrier, retries work, or emits a successor effect. All
three typed failures, immediate and fresh-session ACK, lost-response replay, deadline rollback,
and a direct process settlement-before-ACK crash journey with terminal-confirmed rerun pass
exact-head macOS [run 39081728842](https://github.com/scottopell/phoenix-ide/actions/runs/39081728842)
and full CI [run 39081728855](https://github.com/scottopell/phoenix-ide/actions/runs/39081728855)
at `085c3d95d314531b0bb48dd5440c07b0f56d5533`.

Linux controller all-target tests, doc tests, schema compatibility tests, exact guardian
admission regressions, and all-target Clippy pass in the retained Lima environment. The
new direct-process and launchd journeys pass exact-head hosted macOS run
[`39056571889`](https://github.com/scottopell/phoenix-ide/actions/runs/39056571889)
and full CI run
[`39056573676`](https://github.com/scottopell/phoenix-ide/actions/runs/39056573676)
at `9a25ae6154e418bf94bddd6006b86d97b3ebb55f`. No local launchctl invocation was
performed.

The remaining native contract requires discovered-child birth-token acquisition and
exact-parent adoption, successful-terminal or completed-before-application failed-sample
settlement, child-list failure settlement, signal/mutation dispatch, and production-exclusive
store ownership. Guardian replacement, cross-boot recovery, installer integration, and
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
| REQ-SDC-011 | The standalone `ControllerSqliteStore` owns exact schema-v8 normalized attempt, policy-child, guardian-binding/session/slot, native sampling, child-list, and exact-root identity-observation issue/application/ACK evidence, sampling-failure settlement/confirmation evidence, ordered child-result rows, and reconciliation completion/activation rows with revision compare-and-swap, strict load validation, affine recovery evidence, and fail-stop poisoning. It is distinct from the Phoenix application database. Production-exclusive store ownership remains a gap. |
| REQ-SDC-012 | `SupervisedChild` retains a direct child through original-group finalization. Native disconnect fences, fresh interruption revisions outpace unfinished reconciliation, and the original monotonic deadline refuses expired release/begin. The preceding disposable launchd ownership/activation-only journey passes run `39009502330`; the extended one-sample continuation passes run `39012241514`. Guardian replacement and production recovery remain unresolved risk. |
| REQ-SDC-013 | `driver` issues affine correlated tickets only from committed plans; exhaustive executor, CLI, completion-to-checkpoint, guardian integration, replay, and capacity-reservation coverage pass full CI/macOS at `907ca04670f46c5c2ea9f0b15993a996a83ac911`. |
| REQ-SDC-014 | Safe binary status/closeout, `open_existing`, typed JSON and category exits, and isolated CLI regressions cover missing stores, phase fences, CAS/load poisoning, malformed arguments, and non-native scope; hosted qualification passes at `907ca04670f46c5c2ea9f0b15993a996a83ac911`. |
| REQ-SDC-015 | Separate guardian and runner executables preserve the status/closeout-only safe CLI. The disposable initial runner owns one committed sampling round end to end; distinct replacement modes settle pending ACK, complete two-phase reconciliation, continue one retained sample, or recover one completed-but-unapplied nonterminal sample. The guardian remains the harmless child's original parent. Guardian-only launchd journeys pass runs `39009502330`, `39012241514`, and `39022439278`; a staged two-job guardian/runner lifecycle passes `39027451788`. No production installer or general continuing policy loop exists. |
| REQ-SDC-016 | Owner-only path checks, same-UID peer admission, exact source-input digest Hello identity, protocol-v10 four-byte big-endian bounded typed JSON framing, strict request fields, predecessor-v9/malformed/oversized-frame fencing, and replacement connections are implemented. Earlier protocol-v9 qualification remains historical evidence; v10 exact-head qualification passes macOS run `39081728842` and full CI run `39081728855` at `085c3d95d314531b0bb48dd5440c07b0f56d5533`. |
| REQ-SDC-017 | Explicit wire DTOs remain separate from affine tickets. The native server independently validates durable evidence before consuming its initial gate, entering reconciliation, minting one-use exact sampling admission, verifying a retained issuing-session result for application-only recovery, dropping retained custody after application, or preparing and activating sampling reconciliation. Store evidence plus a correlated same-live response construct a distinct non-dispatch application capability; neither diagnostic source alone can construct it. Live preparation, durable activation, and final live activation each consume a distinct opaque capability; completion/activation rows and wire locators cannot construct them. General mutation remains refused. |
| REQ-SDC-018 | Normalized exact guardian/session/slot binding, binding-before-workload release, persisted liveness refresh, durable fresh interruption, and guardian-validated reconciliation cover bounded sampling, exact-root child listing, and exact-root identity observation. Schema v8 persists normalized issue/application/confirmation evidence, ordered child results, and distinct sampling-failure settlement/confirmation rows. Fresh runners settle sampling, child-list, or identity-observation `AppliedPendingAck` without redispatch; sampling, child-list, and exact-root identity successful retained completions also have distinct completed-before-application capabilities that remain `Interrupted` and emit no successor. Already-durable parked sampling failures can release only result custody while remaining unsettled for reconciliation. Successful-terminal, unapplied-failure, and child-list-failure settlement remain gaps. |
| REQ-SDC-019 | Exact controller-store schema compatibility, source-input build identity, protocol-v10 admission with explicit predecessor-v9 rejection, and platform capability checks exist. Exact schema-v7 stores are rejected unchanged. The transport remains pre-adoption and same-build-only; no upgrade, downgrade, rolling, guardian-replacement, or cross-boot promise is made, and no Phoenix application-DB migration is involved. |
| REQ-SDC-020 | Linux policy/IPC/SQLite tests and macOS process tests cover exact source-input handshake, gated start, one native sample, original parenthood, runner death after application/before ACK, guardian receipt before a lost ACK response, same-live reconnection, durable confirmation with one issue/application, and pending-begin death/catch-up/two-phase activation without another effect; these pass exact-head macOS/full-CI qualification at `55a90279c54ea30c82f3803d80efd340db319a88`. The preceding test-only launchd three-runner application-death/ACK/reconciliation-activation journey and four launchd-free shape/parser units pass run `39009502330` and full CI `39009503934` at `26692b6daef41ef2ae2c4ca80ae17eb87ed07350`. The extended launchd and added direct-process three-barrier/one-sample-continuation journeys, mode-parser regression, and live-Reconciling admission regression pass macOS run `39012241514` and full CI `39012245789` at `1d6f09bb40cf617a0fdbf13275e6297b8d047702`. The new direct and launchd runner-death-before-application journeys, retained-provenance verifier, and interrupted nonterminal application regressions pass macOS run `39022439278` and full CI `39022440430` at `53c45538b98d25e90d749edf72402b001c7f5510`. The staged two-job launchd-owned runner lifecycle passes macOS run `39027451788` and full CI `39027454259` at `5c8526f561659afa206f5a406c05cffeb4107074`. Additional direct and same-label two-job journeys cover completed-before-application child-list recovery, exact-root natural-exit identity reconciliation, zero-signal readiness, and identity application-before-ACK recovery; they pass exact-head macOS run `39056571889` and full CI run `39056573676` at `9a25ae6154e418bf94bddd6006b86d97b3ebb55f`. Already-durable parked sampling-failure settlement, direct crash/ACK-only coverage, and terminal-confirmed rerun pass macOS run `39081728842` and full CI run `39081728855` at `085c3d95d314531b0bb48dd5440c07b0f56d5533`. Successful-terminal and unapplied-failure settlement, child-list failure settlement, discovered-child adoption, signal mutation, installer/helper integration, guardian replacement, updater, and rollback scenarios remain required. |
| REQ-SDC-021 | Schema-v7 child-list issue/application/ordered-result-child/ACK evidence, protocol-v10 exact owned-slot admission, bounded result decoding, same-loop application-before-ACK, and immediate-predecessor rejection are implemented. Direct and two-job launchd journeys cover both completed-before-application and application-before-ACK runner death with one enumeration/application/ACK and no successor effect; ACK/application-only recovery remains `Interrupted`. The new completed-before-application journeys pass exact-head macOS run `39056571889` and full CI run `39056573676` at `9a25ae6154e418bf94bddd6006b86d97b3ebb55f`. No arbitrary PID, signal, cleanup mutation, installer, migration, or production path is authorized. |
| REQ-SDC-022 | Exact-root `ObserveIdentity` uses key/session-only native admission derived from the durable issue and sole PID-plus-birth slot, normalized schema-v7 evidence, same-loop application-before-ACK, original-group finalization, and an already-clean zero-signal deadline transition. Completed-before-application recovery adds protocol-v10 key/session/provenance-only retained-result verification, exactly-once interrupted application, and the existing durable ACK path without re-observation or successor effects. Direct and two-job launchd journeys pass exact-head macOS run `39071653753` and full CI run `39071653757` at `d66f5a5847d8add572fd742c68d25bfef3fde3d1`. Earlier exact-root coverage passes macOS run `39056571889` and full CI run `39056573676` at `9a25ae6154e418bf94bddd6006b86d97b3ebb55f`. |
| REQ-SDC-023 | A sealed validation-only launchd boundary admits the real signed, notarized, receipt-bound PreparedSource114 candidate from an unchanged external artifact, stages it with an internally derived inert predecessor under a generated private root, materializes candidate registry and independent controller helper from immutable commits, constructs and classifies the complete closed manifest with exact timeout and plist label/program bindings, and invokes the read-only helper validator shared with activation. Failure removes the generated root. The executable harness asserts no claim, service control, backup, migration, publication, activation, or installer cleanup. Original exact head `4a73b7ad3f64ad55b99de2d0093c2be62a762a07` passes ARM64 macOS run `39150739258` and full CI run `39150737450`; reconciled hardened exact head `efd277b6051c8096973d26dff501d1ecaee80d3f` passes explicitly dispatched ARM64 macOS run `39164280946` and full CI run `39164278998`. |

## Next work

The implemented test units are the in-process custodian/event loop and a disposable native
transport with durable initial binding/release, one exact sampling application, real runner
death after application/before ACK, a guardian-processed response lost before runner receipt,
same-live replacement-runner ACK confirmation, completed-before-application nonterminal
recovery into interrupted state, pending-begin death/catch-up, and later sampling reconciliation
completion/activation. Direct SQL tests reject FK-clean contradictory
checkpoint/binding/session evidence; failure tests reopen and compare durable rows, including
activation-insert rollback and unsettled-issue recovery refusal. Exact schema-v5 and
immediate-predecessor schema-v7 fixtures check open/open_existing/read-only rejection
without schema or representative-row mutation. Controller-owned schema v8 retains normalized
sampling, child-list, and exact-root identity issue/application/confirmation rows, distinct
sampling-failure settlement/confirmation rows, ordered
child results, and
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
stop/uninstall for fake resources. A successor journey derives ACK-only recovery,
continuation-to-readiness, and terminal completion from durable controller state inside the
same disposable installation owner; it passes exact-head macOS run `39086283270` and full CI
run `39086281970` at `0e89075198623f01c414ebffeaa6fa433e55c562`. Neither journey
demonstrates application deployment.

A separate sealed launchd validation boundary now exercises the real installer-to-helper
contract under a generated owner-private root. It admits the real PreparedSource114 candidate
through receipt/schema, byte-digest, signing, hardened-runtime, notarization, stapling,
Gatekeeper, and exact build-identity checks, derives an inert predecessor witness, and
materializes the candidate registry and independent retained helper from their
immutable commits, generates both plists, constructs the complete PreparedSource114 manifest,
classifies it with production logic, validates its source114 database ledger, and invokes the
helper pre-disruption validator shared with activation. The shared validator closes the exact
PreparedSource114 timeout budgets and plist label/program bindings and is witnessed read-only
across paths, bytes, modes, and ownership. Failed preparation removes its generated root while
leaving the downloaded artifact unchanged. The boundary returns diagnostic evidence only and the
executable harness asserts zero claim, service control, backup, migration, publication, activation,
or installer cleanup.
Exact code head `4a73b7ad3f64ad55b99de2d0093c2be62a762a07` passes ARM64 macOS
run `39150739258` and full CI run `39150737450`. The macOS receipt binds artifact
`12122737139` from preparation run `39089498019` and reports
`claim=0 stop=0 backup=0 migrate=0 publish=0 activate=0 cleanup=0`.

The original receipt-only head `63a26b6f1b2544e85160e52fc7011b61eadf188a`
passes macOS run `39153477397` and full CI run `39153475900`. Latest-main
reconciliation before manual-admission hardening passes at
`6330246f2c383bda060bafbe33d3ea86c6e284b1` with macOS run `39159257402`
and full CI run `39159257367`. Reconciled hardened code head
`efd277b6051c8096973d26dff501d1ecaee80d3f` passes explicitly dispatched
ARM64 macOS run
[`39164280946`](https://github.com/scottopell/phoenix-ide/actions/runs/39164280946)
and full CI run
[`39164278998`](https://github.com/scottopell/phoenix-ide/actions/runs/39164278998).
The manual run binds exact artifact ID, producer workflow source and preparation
mode, verifies all five sealed bundle hashes, passes the real disposable harness,
removes retrieved bytes, and reports
`launchd-validation-only: PASS database_ledger_entries=113 claim=0 stop=0 backup=0 migrate=0 publish=0 activate=0 cleanup=0`.

The remaining replacement gaps are explicit:

- The supported macOS installer remains `dev.py prod deploy --prepared-artifact ...
  --current-schema-upgrade` and its immutable manifest/retained Python helper. The disposable
  boundary validates preparation but cannot acquire a production claim or activate it, and the
  Rust controller does not invoke or replace its signature, claim, application-DB mutation,
  migration, publication, health, or forward-recovery authority.
- `.github/artifacts/prepared-source114-aarch64.json` records ephemeral artifact
  `12122737139`, its API digest, exact producer identity, and expiry on 2026-11-01. No same-byte
  retention, separate host-retention download, republication, or public release is authorized;
  exact hosted validation retrieval requires an explicit authorized workflow dispatch. After
  expiry, an owner-authorized rerun of the pinned historical workflow or equivalently pinned
  reproduction creates a new artifact and new evidence identity that requires a new manifest,
  real byte admission, review, and qualification; it must never impersonate artifact `12122737139`.
- Launchd owns both disposable jobs only inside the test fixture. The disposable installation
  selects its bounded recovery/continuation runner stages from durable state, but no production
  controller exclusively owns the production controller store, supervises finite runners, or
  installs/removes production jobs.
- Native durable dispatch covers resource sampling, exact-root `ListChildren`, and
  policy-issued exact-root `ObserveIdentity`. The natural-exit path reaches
  `ReadyForDisruption` with `VerifiedClean` and no signal; replacement ACK recovery remains
  `Interrupted`. Discovered-child birth-token acquisition, signal mutation, and readiness
  continuation after replacement remain unimplemented.
- The policy core can retain exact candidate commit/digest and enter forward recovery, but the
  macOS adapter cannot reverify candidate bytes and no native runner invokes migration. There
  is no correlation yet with the supported helper's irreversible migration-start evidence.
- Guardian replacement, cross-boot adoption, downgrade, and mixed-build continuation remain
  unpromised. Adding them requires explicit compatibility and custody decisions rather than
  treating a matching label, PID, socket, or copied executable as the original guardian.

The next controller-owned work is successful-terminal and unapplied-failure settlement,
child-list failure settlement, discovered-child identity acquisition, signal
mutation, general continuing orchestration, and the remaining process-death matrix. Those
surfaces must extend REQ-SDC-015–022 and ADR-132–137 while preserving ADR-131
**detach-and-fence**, exact-instance
recovery, the original absolute deadline, retained results, monotonic reconciliation generations,
and whole-round signal-result reservation.

Installer and `dev.py` integration remain separately owned. The validation-only boundary
loads only the supported exact prepared current-schema handoff and shares the helper's
pre-disruption validator; helper-owned claim, recovery, application-DB mutation, publication,
activation, signature, and source-binding policy remain outside the Rust controller.
Independent guardian/runtime work does not wait on artifact identity, and no native activation,
deployment, production adoption, or irreversible helper operation is implemented or authorized
by this qualification.
