# Migrate the native iOS client to ProductConversation

## Outcome

Make the native client consume the stable ProductConversation aggregate instead of treating one transcript row as the user-facing conversation.

## Unblocking evidence

Begin only after the ProductConversation History/delete stack and its client-facing contract are on main. The contract must define durable root identity, Open/History projection, unified transcript boundaries, live-target routing, SSE identity, and supported actions.

## Scope

- Re-ground `ios/PhoenixMobile/Sources/API/` against the shipped REST/SSE contract.
- Re-key list, navigation, snapshots, sessions, and outboxes in `ios/PhoenixMobile/Sources/Store/` to the correct aggregate/member identities.
- Adapt `ios/PhoenixMobile/Sources/Views/` to one ProductConversation entry and one chronological transcript.
- Preserve offline-first durability, idempotent sends, and certificate behavior.
- Add focused contract tests and extend the live simulator journey.

Before implementation, split this umbrella into narrow leaf tasks against the exact shipped contract.

## Acceptance

- One ProductConversation appears once in navigation and remains stable across continuation members.
- Open and History behavior matches the authoritative server projection.
- Cached history, pending messages, and live events reconcile without identity loss or duplicate transcript content.
- The iOS unit suite and live mock-server journey pass.

## Out of scope

Renderer expansion, grounding/files, Markdown fidelity, and prose commenting.
