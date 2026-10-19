# ADR-137: Owned-root child enumeration is distinct native authority

- **Status:** Accepted
- **Date:** 2026-10-17
- **Affects:** REQ-SDC-011, REQ-SDC-013, REQ-SDC-015, REQ-SDC-017–021; native `ListChildren`, controller schema v6, guardian protocol v6

## Context

ADR-133 through ADR-136 establish controller durability, surviving-guardian fencing,
same-build native IPC, and application-only recovery for retained sampling results.
Resource admission already produces an affine `ListChildren` successor, but the native
path admitted only resource sampling. The wire representation of a PID and birth token
was diagnostic data, not proof that the guardian owned that process or that policy had
issued enumeration authority.

Native child enumeration also introduces durable evidence with different semantics
from sampling. A successful result is an ordered PID vector, including a valid empty
vector; an error is not an ownership result. Listed PIDs still lack birth tokens and
cannot be treated as adopted owned identities. Empty enumeration completes one scan but
does not prove that the root is absent or clean.

## Options considered

1. **Permit any well-formed PID/birth identity to request `ListChildren`.** This makes
   wire or database diagnostics sufficient native observation authority and permits
   unrelated-process enumeration.
2. **Generalize all retained native effects into one result blob and generic recovery
   table.** This weakens shape enforcement, duplicates policy meanings, and makes
   child collections document data rather than normalized evidence.
3. **Admit one distinct owned-root operation from the actual affine successor.** Bind
   durable issue evidence to the exact live guardian slot, retain a typed bounded
   completion, apply it through the same policy loop, and acknowledge only after the
   normalized application commits.

## Decision

Use a child-list-specific native authority and durable lifecycle.

The runner consumes the actual `ListChildren` successor issued after resource sampling
and commits an issue containing its exact attempt, revision, ordinal, issuing session,
parent PID/birth identity, and guardian identity. The guardian independently rereads
that issue and requires the current active session, original deadline, exact key and
parent, and one live owned slot whose identity equals that parent before executing one
bounded enumeration. Neither a caller-provided PID, a binding row, a label, nor a socket
path can substitute for those checks.

The controller schema advances from v5 to v6. It stores immutable issue, application,
ordered result-child, and acknowledgement-confirmation rows. Result-child rows preserve
wire order while policy canonicalizes the PID set before discovery. Successful empty
results have zero child rows. Query failures retain typed failure outcomes without
advancing ownership policy or granting acknowledgement. There is no child-result JSON blob,
second deadline, or reconstructed ticket, completion, or policy loop.

The guardian protocol advances from v5 to v6 and rejects v5 peers. Child-list admission,
submission, retained observation, and acknowledgement use distinct typed messages.
Successful completion is applied atomically through the original `PolicyLoop` before
ACK. A replacement session may ACK an already-applied earlier-session result without
re-enumeration or reapplication, but fresh-session establishment interrupts ownership.
ACK-only recovery therefore remains `Interrupted`; resuming cleanup requires the
separate exact-identity reconciliation work.

The bounded no-crash path may finish ownership discovery after an empty root listing and
persist `CleaningOwnedTree`. It stops before dispatching `ObserveIdentity`, signalling,
or cleanup mutation. It does not claim `ReadyForDisruption`.

## Consequences

- Native enumeration authority is limited to the guardian-owned disposable root and the
  exact committed successor; arbitrary PID observation is not introduced.
- Child vectors are bounded on the wire and normalized in SQLite, with durable
  application-before-ACK and idempotent retained-receipt acknowledgement.
- A listed PID remains discovery evidence until birth-token acquisition and exact
  identity observation complete.
- Empty enumeration can end ownership discovery but cannot prove cleanup or readiness.
- Death after durable application can recover the exact ACK without effect replay, but
  cleanup continuation across that death remains behind identity reconciliation.
- The v6 controller store and guardian protocol reject v5 stores and peers before
  mutation; they do not migrate or downgrade them. No mixed-version, production
  installer, application-database, signal, or guardian-replacement guarantee follows.

## Supersession

A later decision may extend native traversal to adopted child identities or resume
cleanup after runner replacement only if each new parent has exact owned-identity
provenance, diagnostic rows remain non-executable, retained results cannot replay, and
fresh identity reconciliation precedes mutation.


## References

- [ADR-136](136_retained-native-results-use-application-only-recovery.md)
- [Standalone deployment controller requirements](../standalone-deployment-controller/requirements.md)
- [Standalone deployment controller behavior](../standalone-deployment-controller/standalone-deployment-controller.allium)
- [Standalone deployment controller status](../standalone-deployment-controller/executive.md)
