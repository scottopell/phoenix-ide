# ADR-102: Destination owner Continue uses explicit local delegation without promoting peers

- **Status:** Accepted
- **Date:** 2026-10-14
- **Affects:** REQ-FOC-001–006, REQ-AUTH-002/009, REQ-PROJ-003/004

## Context

Phoenix can read an enrolled peer through a receiver-issued peer credential, while lifecycle mutations belong to owner authority. A coordinator that resolves every mutation through its local database reports local absence for a remote conversation and never reaches the destination. Reusing peer authentication for Continue would make a peer bearer an owner credential. Copying destination owner secrets into federation persistence would create a second secret authority and expose it to backup, query, and migration surfaces.

Continue also carries more authority than a transcript identifier. Product membership, the exact current exhausted predecessor, mapped WorkScope, Close admission, one-successor arbitration, and the immutable opening intent must all be decided at the destination.

## Options considered

1. **Promote enrolled peer credentials to lifecycle authority** — convenient and requires no owner secret, but contradicts the disjoint owner/peer authority model and makes enrollment grant mutation power.
2. **Persist destination owner credentials with peer connections** — enables unattended transport, but duplicates operator secrets and expands storage, query, backup, and migration exposure.
3. **Delegate one fixed Continue operation through an explicitly configured destination-local owner authority** — uses the peer credential only for caller identity, keeps the owner secret on the destination host, and requires a separate owner-configured caller binding.

## Decision

Choose option 3. The caller's owner-authenticated forward route resolves only the selected enrolled destination's verified HTTPS origin, disables redirects, and calls one fixed delegated peer Continue path. The destination accepts the peer identity only when `PHOENIX_FEDERATION_OWNER_CONTROL_CALLER_INSTANCE_ID` names that exact caller and `PHOENIX_FEDERATION_OWNER_CREDENTIAL_PLIST` names an absolute, size-bounded LaunchAgent plist containing exactly one `PHOENIX_PASSWORD` that matches the server's active owner password. Peer enrollment alone grants no lifecycle authority. The owner credential is neither accepted from request fields, sent over the network, nor persisted by Phoenix. Source authorization, deployment configuration, and successful remote admission remain separate facts.

The destination route requires ordinary owner authentication and a typed request naming destination, product, exact predecessor, expected WorkScope, message identity, and literal handoff. It revalidates those facts under product admission and a checked continuation transaction. Exact retries preserve the persisted winner; changed retries conflict; dispatch failure preserves the successor and durable intent. Dispatch acceptance is not execution completion.

Task approval is not federated by this decision. It remains a separate local owner route with its existing active Git-backed WorkScope, task state, non-subagent, lifecycle, and handoff guards.

## Consequences

- **Positive:** remote Continue cannot silently fall back locally, peer credentials remain non-owner, and destination lifecycle authority stays with the existing transaction.
- **Positive:** Phoenix does not create a persisted copy of destination owner secrets.
- **Negative:** a trusted integration must make the authorized destination owner credential available ephemerally; absent capability or failed authentication stops the operation.
- **Neutral:** source consent identifies an allowed secret source but is not evidence of successful transport or destination admission.

## References

- ADR-034, ADR-067, ADR-083, ADR-092, ADR-093, ADR-094
- `ContinuationApplicationService::continue_owner`, `Database::continue_conversation_with_intent_checked`, `continue_remote_owner`
- [Federation owner control requirements](../federation-owner-control/requirements.md)
- [Authentication requirements](../auth/requirements.md)
- [Bedrock requirements](../bedrock/requirements.md)
