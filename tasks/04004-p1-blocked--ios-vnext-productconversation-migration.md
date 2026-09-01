# Migrate the native iOS client to ProductConversation

## Outcome

Make the native client consume the stable ProductConversation aggregate instead of treating one transcript row as the user-facing conversation.

## Unblocking evidence

Begin only after task 04005's deterministic fixture contract seam is committed and reviewed. Consume the shipped ProductConversation REST aggregate and existing ordinary transcript-row REST/SSE contracts. Permanent delete, follow-up/source retrieval, grounding, and comments are not prerequisites.

## Scope

- Re-ground `ios/PhoenixMobile/Sources/API/` against the shipped REST/SSE contract.
- Re-key user-facing list, navigation, and aggregate snapshots to ProductConversation identity while retaining transcript-row ownership for sessions, SSE, caches, and outboxes.
- Adapt `ios/PhoenixMobile/Sources/Views/` to one ProductConversation entry and one chronological transcript.
- Preserve offline-first durability, idempotent sends, and certificate behavior.
- Add focused contract tests and extend the live simulator journey.

Implement under this umbrella unless two concurrently scheduled workers have genuinely separate source ownership; in that case create at most two leaves for the approved REST/list and live-delegation/composition slices.

## Acceptance

- One ProductConversation appears once in navigation and remains stable across continuation members.
- Open and History behavior matches the authoritative server projection.
- Cached history, pending messages, and live events reconcile without identity loss or duplicate transcript content.
- The iOS unit suite and live mock-server journey pass.

## Out of scope

Renderer expansion, grounding/files, Markdown fidelity, and prose commenting.
