# ADR-132: Retire duplicate naming mutations after caller migration

- **Status:** Accepted
- **Date:** 2026-10-18
- **Affects:** REQ-CID-001, REQ-CID-005, REQ-CHN-007, REQ-COMP-001/002

## Context

ADR-131 established one root title authority but retained legacy mutation delegates. The subsequent scope correction explicitly favors YAGNI after checking real callers. Repository audit identifies browser sidebar/list rename and generated-name actions, persisted offline rename replay, and an unrouted ChainPage management surface. No mutable naming caller was found in the iOS or standalone Python client; external usage is unknown. Historical references depend on read resolution and immutable identities, not mutation URLs.

## Options considered

1. Retain the delegates described in ADR-131. This preserves redundant HTTP contracts and request/response shapes without an identified remaining owned caller.
2. Delete every chain surface. This would incorrectly remove live Q&A and historical read/evidence consumers.
3. Migrate owned naming callers to the ProductConversation operations, retire only duplicate mutations, and retain evidence/read surfaces.

## Decision

Choose option 3. Supersede only ADR-131's compatibility-mutation delegation and legacy override-clear policy. Manual naming uses `PATCH /api/product-conversations/:reference/title`; generated naming uses `POST /api/product-conversations/:reference/regenerate-name`. Both return the authoritative ProductConversation list row and share root identity, title validation, allocation and admission. Generation summarizes the root opening message, pins immutable ownership and the naming revision before awaiting, and cannot overwrite a newer rename.

Remove the transcript rename/regenerate and chain name/regenerate handlers, obsolete request types and uncalled chain-summary generator. Replay persisted offline rename payloads using their immutable transcript reference through the ProductConversation authority. Remove unrouted ChainPage management while retaining its live Q&A component on ProductConversationPage. Do not add synonym routes, alias history, alternate naming state or payload upgrades.

## Consequences

- One owned mutation contract replaces four obsolete routes and their duplicate UI paths.
- Unknown external callers of removed mutations must adopt the canonical operation; no speculative compatibility guarantee is made.
- Read aliases, permanent ProductConversation UUIDs, exact transcript/message references and historical evidence remain unchanged.
- This source/API cleanup authorizes no production migration, data deletion, merge or deployment.

## References

- [ADR-131](131_single-conversation-name-and-allocated-slug.md)
- [Conversation identity requirements](../conversation-identity/requirements.md)
- [Compatibility requirements](../compatibility/requirements.md)
- `regenerate_product_conversation_name`, `rename_product_conversation`, `SyncQueue.processOperation`, `ChainQaColumn`
