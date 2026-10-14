# ADR-102: Destination owner Continue borrows narrow authority without promoting peers

- **Status:** Accepted
- **Date:** 2026-10-14
- **Affects:** REQ-FOC-001–006, REQ-AUTH-002/009, REQ-PROJ-003/004

## Context

Phoenix can read an enrolled peer through a receiver-issued peer credential, while lifecycle mutations belong to owner authority. A coordinator that resolves every mutation through its local database reports local absence for a remote conversation and never reaches the destination. Reusing peer authentication for Continue would make a peer bearer an owner credential. Copying destination owner secrets into federation persistence would create a second secret authority and expose it to backup, query, and migration surfaces.

Continue also carries more authority than a transcript identifier. Product membership, the exact current exhausted predecessor, mapped WorkScope, Close admission, one-successor arbitration, and the immutable opening intent must all be decided at the destination.

## Options considered

1. **Promote enrolled peer credentials to lifecycle authority** — convenient and requires no owner secret, but contradicts the disjoint owner/peer authority model and makes enrollment grant mutation power.
2. **Persist destination owner credentials with peer connections** — enables unattended transport, but duplicates operator secrets and expands storage, query, backup, and migration exposure.
3. **Borrow an explicitly selected destination owner credential for one fixed Continue operation** — keeps owner authentication intact, avoids persistent secret copies, and permits a narrow server-to-server operation at the cost of requiring an external trusted credential source.

## Decision

Choose option 3. The outbound transport binds a borrowed `OwnerPassword` to one selected `InstanceId`, resolves only that enrolled destination's verified HTTPS origin, disables redirects, and calls one fixed owner Continue path. The local forward route obtains that association only when `PHOENIX_FEDERATION_OWNER_CREDENTIAL_INSTANCE_ID` names the requested destination and `PHOENIX_FEDERATION_OWNER_CREDENTIAL_PLIST` names an absolute, size-bounded LaunchAgent plist containing exactly one `PHOENIX_PASSWORD`. The credential is neither accepted from model/tool request fields nor persisted by Phoenix. Source authorization, source availability on the caller host, and successful remote authentication are separate facts; source code and operator consent do not prove that authentication occurred.

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
