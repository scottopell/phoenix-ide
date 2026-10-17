# ADR-120: Exact-root identity observation is distinct native authority

- **Status:** Accepted
- **Date:** 2026-10-17
- **Affects:** REQ-SDC-011, REQ-SDC-013, REQ-SDC-015, REQ-SDC-017–022; native `ObserveIdentity`, controller schema v7, guardian protocol v8

## Context

ADR-119 admits one bounded `ListChildren` operation for the exact guardian-owned root
and deliberately stops at `CleaningOwnedTree`. An empty child list closes discovery but
does not prove that the root is absent. Reaching `ReadyForDisruption` requires fresh
PID-plus-birth observation of every acquired identity and no unresolved discovery.

The generic wire model can describe `ObserveIdentity`, but a description is not native
authority. Accepting a caller identity or enabling generic dispatch would allow a runner
to observe unrelated processes. Native observation also needs durable application and
ACK evidence, and a naturally exited supervised child must be reaped without confusing
an exited leader with a process group that still contains a member.

## Options considered

1. **Accept a caller-provided PID and birth token.** This turns diagnostic wire data into
   authority and can target a process outside the guardian-owned slot.
2. **Infer absence from empty enumeration or slot exit.** Neither proves exact identity
   absence, and an exited leader can still have a live original-group descendant.
3. **Admit only a durable policy-issued exact-root observation.** Derive its identity from
   the issue, require the sole guardian slot and current policy phase, retain the typed
   result, apply it before ACK, and use policy's already-clean deadline transition.

## Decision

Use a distinct native exact-root identity-observation lifecycle.

The controller schema advances from v6 to v7. Immutable normalized issue, typed
application, and ACK-confirmation rows store the exact dispatch key, issuing session,
guardian binding, PID-plus-birth target, observation discriminant and typed payload, and
applying/confirming revisions. The observation is applied through the same `PolicyLoop`
and checkpoint transaction before ACK. No result blob, duplicate deadline, reconstructed
ticket, or generalized observation row is introduced. Schema-v6 stores are rejected
without mutation.

The guardian protocol advances from v7 to v8 and rejects v7 peers. Admission names only
the current session and durable issue key. The guardian rereads the issue and requires
the active session, exact revision and deadline, cleanup or verification phase, same
live guardian identity, and its sole exact PID-plus-birth root slot. Submission executes
one derived `ObserveIdentity`; retained origin and durable application gate ACK.

When the exact supervised root exits naturally, the guardian may reap it only after its
original process group has no member. An exited leader with a retained group member
produces unresolved evidence, never exact absence. This path sends no signal.

After exact absence is durably applied and acknowledged, the runner waits for the
unchanged original deadline. Policy may then enter `ReadyForDisruption` with
`VerifiedClean` and zero effects. A replacement session may recover an ACK from durable
application evidence, but remains `Interrupted`; it does not reconstruct cleanup or
readiness authority.

The same protocol revision also adds application-only recovery for a retained successful
child-list completion. A fresh same-live reconciling session combines the sole pending
issue, exact retained issuing provenance, root slot, and original deadline into a one-use
application capability. It never re-enumerates or reconstructs the original ticket.

## Consequences

- Native identity observation remains limited to the sole exact guardian-owned root and
  a policy-issued cleanup or verification obligation.
- PID hints, wire identities, empty enumeration, and liveness diagnostics remain
  non-authoritative.
- Natural exit can qualify the harmless no-signal path to `ReadyForDisruption` only after
  durable exact-absence application and ACK.
- Death after identity application can recover only the ACK and remains `Interrupted`.
  Completed-before-application identity-observation recovery is not implied.
- Schema v7 and protocol v8 reject their immediate predecessors; no migration,
  downgrade, mixed-version, guardian-replacement, installer, application-database,
  production signaling, or production deployment guarantee follows.

## Supersession

A later decision may admit discovered child identities or process mutation only after
birth-token acquisition and exact-parent evidence create owned identities, every native
operation has its own durable authority, and fresh reconciliation prevents stale or
caller-selected identity from becoming mutation authority.
