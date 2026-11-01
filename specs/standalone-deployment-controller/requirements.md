# Standalone Deployment Controller

## User Story

As the operator responsible for replacing a running Phoenix installation, I need
deployment orchestration to make resource, ownership, cleanup, and recovery
decisions from durable typed evidence rather than shell-process accidents, so an
interruption or PID race cannot silently turn uncertainty into permission to
continue.

## Domain

The standalone controller is an independent policy core. It owns deployment
attempt state and emits typed operating-system requests only after producing the
checkpoint that must be durable first. Platform adapters supply process identity,
process-tree observations, resource measurements, signals, and candidate artifact
verification. The controller has no implicit authority over launchd or
production-deployment entry points; each entry point can use it only through an
explicit adapter.

The controller refines the compatibility policy in
`specs/compatibility/requirements.md`, the independent-owner boundary in
`specs/production-deployment/requirements.md`, and launchd activation behavior in
`specs/launchd-deployment/requirements.md`. It does not create a downgrade,
database rollback, or live-resource replacement guarantee. The native runner and
guardian contract is limited to disposable, same-build pre-adoption qualification;
it grants no production adoption or installer integration authority.

---

## Requirements

### REQ-SDC-001: Durable Phase and Effect Ordering

WHEN a deployment attempt changes phase or records new evidence
THE SYSTEM SHALL produce a complete attempt checkpoint before producing any
operating-system mutation requested by that transition.

WHEN execution is interrupted after a checkpoint
THE SYSTEM SHALL recover from that checkpoint without assuming that an emitted
operating-system request completed.

WHEN a checkpoint has an unfinished child-enumeration, identity-observation, birth-probe,
or resource-sample request
THE SYSTEM SHALL retain and reissue that exact obligation during recovery before
advancing past its owning phase.

---

### REQ-SDC-002: Measured Resource Admission

WHEN the controller decides whether resource conditions admit deployment work
THE SYSTEM SHALL use an ordered configured window of measured capacity and
contention samples.

THE SYSTEM SHALL NOT use process names, a process blacklist, or equality of a
global process-table snapshot as resource-admission authority.

WHEN contention exceeds policy in only part of the required window
THE SYSTEM SHALL NOT classify that transient sample as sustained contention.

---

### REQ-SDC-003: Stable Process Identity

WHEN the controller records an owned process
THE SYSTEM SHALL identify it by native PID and a high-resolution process birth
token.

WHEN a PID appears in a child listing but no birth token has been acquired
THE SYSTEM SHALL record a distinct discovery epoch rather than an owned process.

WHEN a discovery epoch has recorded any failure before birth identity acquisition
THE SYSTEM SHALL NOT permit a later process at that PID to satisfy that epoch,
even when a reconstructed platform adapter observes the expected parent relationship.

---

### REQ-SDC-004: Truthful Observation Uncertainty

WHEN birth-identity acquisition reports no-such-process, permission denial,
temporary unavailability, or malformed metadata
THE SYSTEM SHALL retain the discovery as unresolved typed evidence
AND SHALL NOT convert that observation into verified process absence.

WHEN an unresolved discovery reaches the attempt's absolute operation deadline
THE SYSTEM SHALL stop bounded reconciliation in an
`administrative_closeout_required` phase rather than retrying indefinitely or
claiming verified cleanup.

---

### REQ-SDC-005: Exact Owned-Tree Mutation

WHEN the controller requests a process signal
THE SYSTEM SHALL name an acquired PID-plus-birth identity that a fresh
observation still matches.

WHEN an owned process is reparented after identity acquisition
THE SYSTEM SHALL retain ownership of that exact identity.

WHEN an owned PID is reused by a different birth identity
THE SYSTEM SHALL treat the original identity as absent
AND SHALL NOT signal or adopt the replacement without separate ownership evidence.

---

### REQ-SDC-006: Verified Cleanup

WHEN the controller reports `verified_clean`
THE SYSTEM SHALL have verified absence of every acquired owned identity
AND SHALL have no unresolved discovery epoch.

AN empty PID scan, a missing process group, or an identity probe that failed before
birth acquisition SHALL NOT by itself establish verified owned-tree cleanup.

---

### REQ-SDC-007: Administrative Closeout

WHEN explicit administrative risk acceptance identifies an attempt in
`administrative_closeout_required`
THE SYSTEM SHALL close administrative scheduling ownership without claiming
verified cleanup, requesting another process signal, or resuming that deployment.

THE accepted record SHALL retain the unresolved ownership evidence that required
administrative closeout.

---

### REQ-SDC-008: Interruption Reconciliation

WHEN a pre-migration attempt recovers from interruption
THE SYSTEM SHALL freshly observe every acquired owned identity before requesting a
new mutation
AND SHALL preserve the original absolute operation deadline.

THE SYSTEM SHALL NOT request any owned-process mutation until every acquired owned
identity has completed that recovery observation barrier.

WHEN reconciliation is itself interrupted
THE SYSTEM SHALL retain the original resume target and restart the complete
observation barrier.

WHEN reconciliation completes at or after the absolute operation deadline
THE SYSTEM SHALL NOT restore process-mutation authority unless cleanup is already
verified clean.

THE SYSTEM SHALL NOT replay a signal solely because the interrupted checkpoint
shows that the signal was planned or emitted.

---

### REQ-SDC-009: Forward Recovery Boundary

WHEN candidate migration invocation has been durably recorded
THE SYSTEM SHALL recover only by reverifying and running the exact retained
candidate identified by full source commit and artifact digest.

THE SYSTEM SHALL NOT start the predecessor or imply that an older binary is
compatible with the migrated database.

WHEN candidate migration invocation is already durable
THE SYSTEM SHALL reject an attempt to replace its retained candidate identity
AND SHALL allow recovery to reissue exact-candidate verification from both the
ready migration checkpoint and a forward-recovery checkpoint.

---

### REQ-SDC-010: Platform Boundary and Qualification

THE policy core SHALL receive process and resource facts through a typed
operating-system boundary
AND SHALL keep durable telemetry free of command lines, environment values,
credentials, and raw secret-bearing operating-system errors.

WHEN controller behavior is qualified
THE SYSTEM SHALL cover deterministic policy and race cases with a fake operating
system
AND SHALL separately qualify platform adapters with harmless isolated native
labels and resources before production adoption.

---

### REQ-SDC-011: Independent Normalized Persistence

THE standalone controller SHALL own a SQLite database whose lifecycle is independent
of the Phoenix application database under migration.

THE controller database SHALL normalize attempts, resource samples, owned identities,
pending child scans, discovery epochs, retained candidates, guardian bindings,
controller sessions, retained child inventory, and effect-result custody with
relational constraints rather than storing those child collections in an aggregate
blob. Native binding and acknowledgement obligations are defined in REQ-SDC-018.

WHEN controller SQLite authority cannot classify whether a checkpoint committed
THE SYSTEM SHALL fail stop rather than publish effects or continue admission.

WHEN a checkpoint replaces an existing attempt row
THE SYSTEM SHALL compare the checkpoint's durable revision with the stored revision
in the same transaction
AND SHALL reject stale or same-ID replacement authority.

WHEN a checkpoint is loaded after controller restart
THE SYSTEM SHALL return affine recovery evidence that cannot request process mutation
until the required interruption and fresh-reconciliation checkpoints are committed.

WHEN persistence or load authority cannot classify durable state
THE SYSTEM SHALL invalidate that database handle for further admission until the
controller reopens the database and classifies its durable evidence.

---

### REQ-SDC-012: Owned-Child macOS Mutation

WHEN the macOS adapter creates process-mutation authority
THE SYSTEM SHALL spawn a direct child as an isolated process-group leader and retain
that child unreaped until group cleanup and final wait.

THE general observation adapter SHALL NOT signal an arbitrary process solely from a
PID-plus-birth observation.

THE SYSTEM SHALL expose descendant group escape, deliberate same-session group join,
and controller exit as unresolved supervision risks
AND SHALL treat supervision lost across controller exit as unresolved risk unless a
surviving supervisor handoff is positively identified.

WHEN a launchd-owned guardian has retained supervised children and its policy
controller exits
THE SYSTEM SHALL fence the ended controller session while allowing only already
started child work to continue
AND SHALL retain child results and native ownership without starting another step or
sending another signal.

WHEN a replacement policy controller reconnects
THE SYSTEM SHALL verify the same live guardian instance and its retained child slots
AND SHALL commit interruption and fresh reconciliation before authorizing any new
mutation
AND SHALL preserve the original absolute operation deadline.

WHEN the original deadline expires before recovery completes, or when the retained
guardian instance is lost or replaced,
THE SYSTEM SHALL preserve unresolved supervision risk and SHALL NOT adopt historical
PIDs, process groups, socket paths, labels, or heartbeat expiry as mutation authority.

---

### REQ-SDC-013: Affine Dispatch and Completion Correlation

WHEN a transition has successfully persisted its checkpoint
THE SYSTEM SHALL allow consumption of that `CommittedTransition` to mint one
non-cloneable `DispatchBatch` of non-cloneable `DispatchTicket` values.

EACH ticket SHALL carry exactly one `OsEffect` and a correlation key consisting
of the attempt ID, the issuing committed persistence revision, and the effect's
zero-based ordinal in the committed plan. Sibling effects SHALL have distinct
ordinals, including when their effect values are equal.

THE SYSTEM SHALL prevent executable ticket or batch fabrication from diagnostic
checkpoint or effect values. Uncommitted transitions, failed persistence, and
crash-loaded evidence SHALL expose no dispatch authority. Consuming a committed
plan through a diagnostic decomposition SHALL discard its dispatch authority.

`AttemptStore` implementations used by the dispatch boundary SHALL be trusted durable
issuers and SHALL return success only after committing the supplied checkpoint.
Dispatch tickets SHALL gate admission to `EffectExecutor`; `ControllerOs` remains an
adapter interface rather than a global operating-system capability sandbox.

WHEN the executor consumes one ticket
THE SYSTEM SHALL make exactly one matching `ControllerOs` call and return an
`ExecutedEffect` that structurally binds the original key and effect identity to
the corresponding typed `EffectCompletion` outcome, including typed failures.

THE executor SHALL NOT retry, escalate, roll back, infer phase, apply results to
policy state, fabricate evidence, or authorize activation. Exact retained-candidate
reverification SHALL remain an observation, not activation permission. Dropping a
batch or unexecuted ticket SHALL make no operating-system call.

THE dispatch boundary SHALL NOT claim exactly-once execution across controller
crash or derive replay permission from an issued ticket.

---

### REQ-SDC-014: Safe Standalone Store Commands

THE `phoenix-deployment-controller` safe store binary SHALL accept only
`status --store PATH --attempt U128` and
`closeout --store PATH --attempt U128`, with each flag supplied once in either
order. The attempt ID SHALL be unsigned ASCII decimal within the full `u128`
range. Unknown commands, unknown or duplicate flags, missing values, and malformed
IDs SHALL fail before opening a store.

THE commands SHALL open only an existing controller store without creating a
missing database or initializing an unversioned one. Missing stores SHALL remain
absent. The commands SHALL preserve schema admission, compare-and-swap revision
checks, and load/persistence fail-stop poisoning.

WHEN `status` loads an attempt
THE SYSTEM SHALL consume only affine loaded evidence for inspection and print
one JSON object followed by a newline without changing the checkpoint or requesting
recovery. The summary SHALL contain only `attempt_id` (canonical decimal string),
`persistence_revision` (positive integer), `phase` (the snake-case phase name), and
`cleanup_outcome` (the snake-case cleanup outcome name), without exposing a live
checkpoint, effects, or native process/resource details.

WHEN `closeout` loads an attempt in `administrative_closeout_required`
THE SYSTEM SHALL consume its stable evidence, create matching
`AdministrativeRiskAcceptance`, persist the administrative closeout, verify that
the committed batch has zero effects, and report `administratively_closed` through
the same summary shape. Administrative closeout SHALL preserve unresolved evidence
and SHALL NOT claim verified cleanup or authorize deployment continuation.

WHEN `closeout` loads any other phase
THE SYSTEM SHALL fail before recovery preparation or persistence without changing
the durable revision or executing an effect.

THE binary SHALL NOT expose native run, reconcile, deploy, activation, installer,
or schema-migration commands, and SHALL NOT invoke a native deployment adapter.

THE commands SHALL emit success JSON only on stdout and fixed category diagnostics
only on stderr. Diagnostics SHALL NOT include arguments, paths, raw SQL or SQLite
errors, command lines, or environment values. Exit codes and stderr categories
SHALL be:

| Exit code | Meaning | Stderr category |
| --- | --- | --- |
| 0 | Successful summary | None |
| 2 | Invalid command or arguments | `usage_error` |
| 3 | Store open, schema, load, or persistence failure | `store_error` |
| 4 | Attempt not found in the existing store | `attempt_not_found` |
| 5 | Closeout rejected for the loaded phase | `wrong_phase` |
| 6 | Internal invariant, JSON encoding, or stdout I/O failure | `internal_error` |

---

### REQ-SDC-015: Separate Native Custodian and Runnable Policy Owner

THE SYSTEM SHALL provide separate guardian and runner executables from the same
build, distinct from the REQ-SDC-014 safe store binary. The safe binary SHALL remain
status/closeout only; native execution SHALL NOT become another command of it.

THE native runner SHALL own the runnable policy loop, controller-store commits,
effect issuance, completion application, and durable acknowledgement. The guardian
SHALL remain an effect custodian rather than choosing policy transitions, retries,
escalation, rollback, activation, or migration compatibility.

THE disposable macOS launchd job SHALL own the guardian independently of the runner.
THE guardian SHALL be the original direct parent of the harmless workload and retain
its native child/group ownership and results across runner death. It SHALL NOT adopt
an already-running process from a stored PID, group, label, or socket path.

THE runnable loop SHALL be independently invocable without installer or `dev.py`
orchestration and SHALL use only isolated disposable resources and harmless work.
It SHALL NOT invoke the prepared deployment helper, alter production labels, mutate
the Phoenix application database, install artifacts, or activate production services.
This contract SHALL NOT transfer installer, claim, ledger, signature, source-binding,
or schema-migration policy to the runner or guardian.

---

### REQ-SDC-016: Bounded Owner-Only Native IPC

THE runner and guardian SHALL communicate over an owner-only Unix-domain stream
socket in an owner-only directory. The guardian SHALL verify the local peer's owner
identity before session admission; pathname discovery alone SHALL NOT authenticate a
peer or establish guardian continuity. This is a trusted same-owner boundary, not a
sandbox against arbitrary code executing as that owner.

EACH frame SHALL contain a four-byte unsigned big-endian payload length followed by
one UTF-8 typed JSON message. Both peers SHALL reject zero-length and over-limit
frames before payload allocation, using a finite shared build-defined maximum, and
SHALL reject malformed, truncated, unknown-variant, or schema-invalid messages without
executing their requested effect. Partial reads and writes SHALL preserve framing;
EOF or protocol failure SHALL fence the associated session, not trigger cleanup.

THE handshake SHALL check exact build identity, protocol version, attempt identity,
and live guardian identity before opening a session. A mismatch SHALL fail closed,
without negotiation, fallback decoding, session replacement, or process mutation.
The protocol SHALL carry explicit request/response DTOs for handshake, inventory,
admission, dispatch, retained results, reconciliation, and acknowledgement, with typed
failures and exact correlation rather than raw operating-system error strings.

---

### REQ-SDC-017: Wire Data Is Not Affine Authority

THE SYSTEM SHALL define wire DTOs separately from internal affine authority types.
Deserializing a request or response SHALL NOT construct internal affine authority,
including `CommittedTransition`, `DispatchBatch`, `DispatchTicket`,
`SessionDispatchTicket`, `ExecutedEffect`, `ReconciledSessionPermit`, or
`AppliedResultAck`. Serializable keys, effect descriptions, revisions, and diagnostic
evidence SHALL NOT independently mint execution, recovery, or acknowledgement authority.

WHEN the trusted runner consumes a committed affine ticket to request dispatch
admission over the authenticated session
THE guardian SHALL validate the live binding, dispatch revision floor, recovery gate,
and required result capacity before minting an opaque one-use admission. The admission
SHALL be bound to that exact live guardian instance, controller session, attempt,
`DispatchKey`, and complete typed effect including every input, not just its kind.
The guardian's live admission registry, not a deserialized token, SHALL own admission.

WHEN a dispatch request presents an admission
THE guardian SHALL consume its matching live registry entry before calling the native
executor. An unknown, spent, fenced, wrong-instance, wrong-session, wrong-key, or
wrong-effect admission SHALL produce no native call. Duplicate dispatch SHALL NOT
repeat execution; uncertain delivery SHALL be resolved through retained results and
fresh reconciliation, never automatic dispatch replay.

THE guardian SHALL reserve result custody for the entire mutation round before its
first effect executes. Admission refusal, oversize rounds, and loss of authority
before execution starts SHALL emit no mutation. Once execution has started,
interruption SHALL retain executed results without inferring permission to finish
the round.

THE native boundary SHALL preserve the one-call correlation and no-activation limits
of REQ-SDC-013 and SHALL NOT claim exactly-once execution across process crash.

---

### REQ-SDC-018: Durable Native Binding, Custody, and Reconnect

THE controller store SHALL bind each attempt to the guardian's instance nonce and
native PID/birth identity, controller-session identity and fencing state, exact child
slot inventory with native identities, dispatch/reconciliation revision floors, and
retained result identities and application/acknowledgement state. These values SHALL
occupy normalized columns and child rows with foreign keys, uniqueness, ordering
where meaningful, and shape constraints; inventory and result lists SHALL NOT be
serialized into a checkpoint blob. Any new timestamp columns SHALL obey
`specs/compatibility/requirements.md` REQ-COMP-005.

THE runner SHALL durably establish the binding before authorizing native workload
execution. Persisted rows SHALL be evidence only: reopening the store SHALL NOT
reconstruct native parenthood, executable tickets, live admissions, or a live session.
Unknown commit outcomes SHALL preserve the REQ-SDC-011 fail-stop boundary.

WHEN a runner disconnects or dies
THE guardian SHALL fence that session and all its unused admissions, retain original
child ownership and unacknowledged results, and allow only already-started work to
continue. It SHALL NOT start another step or signal on EOF, heartbeat expiry, timeout,
or a later connection. A second connection SHALL NOT silently replace an active owner.

WHEN a runner reconnects
THE SYSTEM SHALL positively verify the same live guardian, exact retained child slot
inventory and native identities, and the original absolute deadline against durable
binding evidence before establishing a fresh session. An inventory mismatch, guardian
loss/replacement, or expired deadline SHALL fail stopped with unresolved supervision
risk, not silently repair the binding or adopt a process. Child completion may change
typed liveness/results but SHALL NOT substitute a slot or its original identity.

THE recovering runner SHALL commit a fresh interruption revision strictly newer than
all prior reconciliation sessions, including interrupted ones, then complete and
commit fresh reconciliation of every acquired identity before any new mutation.
Retained results SHALL remain evidence, not a substitute for that observation barrier;
dispatch floors SHALL NOT regress and the original deadline SHALL NOT be extended.

WHEN a runner crashes after committing interruption but before the guardian accepts that
interruption for a new reconciliation session
THE recovering runner SHALL consume freshly crash-loaded Interrupted evidence to commit a
zero-effect interruption refresh at a strictly newer revision, preserving the original
interruption state and deadline without granting recovery, reconciliation, mutation, or
acknowledgement authority. Non-Interrupted evidence SHALL NOT authorize this refresh.

WHEN a retained completion is received
THE runner SHALL correlate its instance, session of issuance, key, full effect, and
typed outcome, and durably record its policy application or explicit round settlement
before sending ACK. ACK SHALL name the exact retained result and committed applying
revision; the guardian SHALL validate it against that result's binding before dropping
custody. A recovered session MAY acknowledge an earlier session's result only after
verified same-instance reattachment and durable application evidence, including while fresh
reconciliation is incomplete; such acknowledgement SHALL NOT restore mutation authority.
Receipt, socket delivery, a claimed revision alone, or a failed/ambiguous commit SHALL
NOT authorize ACK.
A lost ACK SHALL permit re-reading retained evidence and re-acknowledging the same
durable application, not reapplying it or reexecuting the effect.

WHEN the same live guardian retains a successful resource sample that the issuing
runner did not durably apply
THE recovering runner MAY apply that retained sample only after committing a fresh
interruption and establishing an exact reconciling session before the original
absolute deadline. The application authority SHALL require the sole durable
`IssuedPendingApplication` issue, exact issuing-session provenance retained by the
guardian, the current interrupted resource-sampling obligation, and a correlated
live response from that guardian; none of those evidence sources alone SHALL mint
application authority.

THE recovered application SHALL atomically append only a nonterminal sample to the
interrupted resource-sampling resume state and record the exact native application.
It SHALL leave the attempt interrupted, return no executable plan, and grant no
dispatch, reconciliation activation, process mutation, or acknowledgement authority.
The guardian SHALL retain custody when the sample is failed, terminal, stale,
ambiguous, mismatched, or admitted at or after the original deadline.

WHEN that recovered application is durable
THE fresh reconciling session MAY acknowledge the exact earlier-session result under
the ordinary durable-application proof. A crash or lost response before application
commit SHALL permit fresh verification of retained custody, not effect reexecution;
a crash after application commit SHALL use acknowledgement-only recovery.
WHEN the sole exact retained sampling result is an already-durable `ParkedRetained`
typed observation failure
THE runner MAY explicitly settle only that failed result after a fresh interruption and
same-live reconciling session before the original deadline. Settlement authority SHALL
bind the exact issue, sequence, failure application and applying revision, issuing-session
provenance retained by the guardian, unchanged guardian identity and slot inventory, and
a correlated live failure response. The request SHALL NOT accept a caller-selected
failure, process identity, or effect as authority.

THE settlement SHALL atomically advance only the interrupted checkpoint revision and
insert normalized immutable settlement evidence after deadline checks under writer
ownership and immediately before commit. It SHALL preserve the interrupted sampling
obligation, policy state, owned tree, guardian binding, slots, and original deadline; emit
no effect; and authorize neither retry, continuation, resource admission, child listing,
identity observation, signal, cleanup, nor readiness. A settled failure SHALL remain
unsettled for reconciliation.

WHEN exact failure settlement is durable
THE guardian MAY release only that retained failed result after independently rereading
and matching the settlement, retained typed failure, issuing provenance, current same-live
reconciling session, and original deadline. ACK confirmation SHALL use a distinct
normalized row rather than successful-sample ACK evidence. A lost ACK response or crash
after settlement SHALL permit ACK-only replay in the same settling session or a later
fresh interrupted reconciling session; it SHALL NOT repeat native work or settlement.
Failed results without durable application and successful terminal samples remain retained.


WHEN the same live guardian retains a successful exact-root child enumeration that the
issuing runner did not durably apply
THE recovering runner MAY obtain a distinct application-only capability after the same
fresh interruption, exact reconciling-session, issuing-provenance, slot, deadline, and
live-result checks. That capability SHALL atomically append the exact child-list result
to the interrupted ownership resume state and normalized result rows, preserve received
PID order while policy canonicalizes discovery hints, leave the attempt interrupted,
and return no executable plan. It SHALL NOT reconstruct the original dispatch ticket,
re-enumerate the root, activate reconciliation, observe identity, mutate a process, or
itself authorize ACK. A pre-commit crash MAY repeat live verification; a post-commit
crash SHALL use acknowledgement-only recovery.

---

### REQ-SDC-019: Pre-Adoption Same-Version Boundary

THE native protocol and controller schema SHALL support only the exact same build
and admitted schema/protocol versions for disposable pre-adoption execution. Unsupported
versions SHALL be rejected before mutation. There SHALL be no controller schema-v1
upgrade guarantee, downgrade guarantee, rolling/mixed-version interoperability,
live guardian replacement, or recovery guarantee across guardian death, logout,
reboot, or cross-boot deadline interpretation. A new disposable store SHALL NOT erase
or reinterpret an incompatible store as if its attempt had recovered.

THE SYSTEM SHALL NOT infer a Phoenix application-database migration guarantee from
controller-store handling or extend the compatibility guarantees in
`specs/compatibility/requirements.md`. Same-build runner restart with the same live
guardian under REQ-SDC-018 is the complete native reconnect guarantee.

---

### REQ-SDC-020: Disposable Native Death and Reconnect Qualification

THE native qualification suite SHALL run separate real guardian and runner processes
with a disposable launchd job, owner-only socket, disposable controller store, and
harmless guardian-parented workload, without production labels, application databases,
installation paths, or deployment helpers.

THE suite SHALL kill the runner while work is live, prove the same guardian and
original child survive with no new autonomous step or signal, observe retained
completion after detach, and reconnect a fresh runner through durable interruption
and complete fresh reconciliation within the original deadline. It SHALL also cover
runner death after result receipt but before application commit, after commit but
before ACK, and lost ACK, proving retained custody and no blind effect replay.

THE suite SHALL reject stale sessions, spent/wrong-bound admissions, malformed and
oversize frames, mismatched builds/protocol/schema, mismatched inventory, replaced or
dead guardians, and recovery after the original deadline. It SHALL verify fail-stop
persistence behavior and whole-round capacity refusal without partial mutation.

THE SYSTEM SHALL report deterministic in-process qualification separately from native
process-death/reconnect qualification. Passing either SHALL NOT constitute production
adoption, installer integration, deployment authorization, or proof of exactly-once
execution across crash.

---

### REQ-SDC-021: Exact Native Owned-Root Enumeration

WHEN resource admission produces a child-enumeration obligation for the harmless
supervised workload
THE runner SHALL durably issue the exact committed `ListChildren` effect before
native submission, and the guardian SHALL independently verify the current session,
revision, key, deadline, parent identity, durable issue, and exact guardian-owned
root slot before one bounded enumeration. A caller-provided PID/birth pair, wire DTO,
label, socket path, or persisted binding row alone SHALL NOT authorize enumeration.

WHEN native child enumeration completes
THE runner SHALL correlate the exact retained typed result, preserve its complete
received PID order as durable evidence, and atomically apply its canonicalized PID set
through the same policy loop before acknowledgement. The controller store SHALL retain
normalized issue, application, ordered result-child,
and acknowledgement evidence without a result blob or duplicate deadline. Failed,
malformed, stale, ambiguous, mismatched, or expired results SHALL remain retained or
fail stopped without advancing ownership policy or authorizing acknowledgement.

WHEN successful enumeration returns no child PIDs
THE SYSTEM MAY complete that root's ownership-discovery scan and enter owned-tree
cleanup, but SHALL NOT treat emptiness as proof that the root is absent, clean, or
ready for disruption. A listed PID remains discovery evidence until a birth token is
acquired and exact identity observation adopts it into the owned tree.

WHEN a runner dies after child-list application but before acknowledgement
A same-live replacement session MAY acknowledge the exact retained result after
re-reading durable application evidence without re-enumerating or reapplying it.
Establishing that fresh session interrupts the ownership checkpoint; acknowledgement
alone SHALL NOT reconstruct a policy loop, resume cleanup, authorize identity
observation, or claim readiness for disruption.

---

### REQ-SDC-022: Exact Native Owned-Root Identity Observation

WHEN owned-tree cleanup or verification produces an exact identity-observation
obligation for the harmless supervised root
THE runner SHALL durably issue that committed `ObserveIdentity` effect before native
submission, and the guardian SHALL derive the target from the durable issue and verify
its current session, key, revision, original deadline, guardian identity, policy phase,
and sole exact owned-root slot including PID and birth token. The native request SHALL
NOT accept a caller-selected PID, birth token, parent, or generalized effect as
observation authority.

WHEN native identity observation completes
THE runner SHALL correlate the exact retained typed result and atomically apply it
through the same policy loop before acknowledgement. The controller store SHALL retain
normalized immutable issue, typed application, and acknowledgement evidence without a
result blob or duplicate deadline. Malformed, stale, ambiguous, mismatched, or expired
evidence SHALL remain retained or fail stopped without advancing cleanup policy or
authorizing acknowledgement. A
correlated typed `Unresolved` result MAY be durably applied and acknowledged as
uncertainty, but SHALL NOT advance cleanup, authorize mutation, or establish readiness.

WHEN the supervised root exits naturally and its original process group has no retained
member
THE guardian MAY reap that exact supervised child before observing the issued identity.
An exited leader whose original process group still has a member SHALL remain unresolved
and SHALL NOT become exact-absence evidence. No signal or other cleanup mutation is
authorized by this observation path.

WHEN exact absence or valid PID reuse is durably applied and acknowledged before the
original deadline
THE runner MAY wait for that unchanged deadline and ask policy to settle cleanup. Policy
MAY enter `ReadyForDisruption` with `VerifiedClean` only when every acquired identity is
verified absent and no discovery obligation remains. This already-clean deadline path
SHALL issue no `SignalExactIdentity` effect. Empty child enumeration, slot liveness,
process exit alone, or a PID without its acquired birth token SHALL NOT authorize
readiness.

WHEN a runner dies after identity-observation application but before acknowledgement
A same-live replacement session MAY acknowledge the exact retained result from durable
application evidence without repeating observation or reconstructing a policy loop. The
fresh session interrupts the checkpoint, so acknowledgement-only recovery SHALL remain
`Interrupted` and SHALL NOT claim cleanup completion or readiness.

WHEN the same live guardian retains a completed exact-root identity observation that the
issuing runner did not durably apply
THE recovering runner MAY obtain a distinct application-only capability only after a
fresh interruption and exact reconciling session before the original absolute deadline.
The authority SHALL require the sole durable `IssuedPendingApplication` issue, exact
issuing-session provenance retained by that guardian, the guardian's unchanged sole
PID-plus-birth root slot, and a correlated live response; the recovery request SHALL NOT
accept a caller-selected PID or birth token.

THE recovered application SHALL atomically apply the exact retained typed observation
to the interrupted ownership resume state and normalized application row, leave the
attempt `Interrupted`, and return no executable plan, dispatch, reconciliation activation,
process mutation, cleanup continuation, readiness, or acknowledgement authority. It
SHALL NOT reconstruct the original dispatch ticket or repeat identity observation.
Deadline expiry after writer ownership or before commit SHALL roll back without policy
advance. A pre-commit crash MAY repeat live verification; a post-commit crash SHALL use
acknowledgement-only recovery.


## REQ-SDC-023 — Disposable installer validation is structurally pre-disruption

WHEN validating the controller-to-launchd-installer boundary before production adoption
THE system SHALL generate an owner-private temporary root. Its sole external artifact input
SHALL pass the exact receipt commit/schema, candidate-byte digest, Developer ID signature,
hardened-runtime, notarization, stapling, and Gatekeeper checks before candidate execution;
the candidate's build-identity execution SHALL then complete exact commit/version admission.
The system SHALL derive its predecessor witness, disposable service label, transaction paths,
target paths, evidence paths, and health endpoints internally rather
than accepting production locations or executable fixtures from the caller.

THE boundary SHALL validate one complete closed installer handoff using the same manifest
loader, exact mode classifier, identity checks, immutable candidate migration registry,
database-ledger admission, and pre-disruption helper validator used by activation. Candidate
registry evidence SHALL come from the exact candidate source commit; retained helper evidence
SHALL come from the distinct clean controller commit. The exact PreparedSource114 handoff SHALL
use the build-defined transition and health timeout budgets. Missing, extra, malformed,
source-mismatched, ledger-mismatched, altered-timeout, or plist-to-manifest-inconsistent
evidence SHALL fail closed before service disruption, and helper validation SHALL leave all
retained evidence unchanged.

IF validation fails after generating the private root
THEN the system SHALL remove only that generated root before returning the failure and SHALL
leave the external prepared artifact unchanged.

A successful validation SHALL return diagnostic evidence only. It SHALL NOT acquire or release
a production claim, prepare a target publication, reserve migration capacity, stop or start a
service, create a database backup, write a migration marker, invoke migration, publish an
artifact or plist, perform health activation, finalize or recover a transaction, signal a
process, clean production evidence, or authorize guardian replacement. The validation path
SHALL be unable to construct the service-control, database-mutation, publication, activation,
cleanup, or guardian-replacement capabilities owned by those operations.
