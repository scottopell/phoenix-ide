# B1: Ship the ProductConversation API/list shared seam for umbrella 04004

## Parent

Parent umbrella: `tasks/04004-p1-blocked--ios-vnext-productconversation-migration.md`

## Why this leaf exists

Worker B1 is the exclusive shared-contract owner for the iOS ProductConversation migration. The current native client still keys its user-facing list, cache, notifications, and coordinator memory by transcript-row `Conversation.id`, while the normative migration requires durable ProductConversation aggregate identity for user-facing list/navigation surfaces and preservation of transcript-row ownership only for live sessions, SSE, cache snapshots, and outboxes.

Task 04005 is complete and explicitly authorizes core live migration to begin after its typed fixture contract seam is committed and reviewed. That fixture contract is authoritative but does not own ProductConversation lifecycle, delete, provenance, live-routing, or persistence behavior, so B1 must establish the shared API/model/cache seam before B2 depends on it.

## Exact scope owned by B1

B1 owns only the shared contract seam and its focused validation:

- `ios/PhoenixMobile/Sources/API/Models.swift`
  - Add the canonical ProductConversation REST DTO / wire-model types consumed by native code.
  - Keep transcript-row message/session types separate; do not collapse aggregate identity into transcript identity.
- `ios/PhoenixMobile/Sources/API/PhoenixAPI.swift`
  - Update list/detail/coordinator REST decode surfaces to the shipped aggregate contract.
  - Expose the canonical aggregate route and identity symbols that downstream code should consume.
- `ios/PhoenixMobile/Sources/Store/ConversationListStore.swift`
  - Re-key the cached user-facing list to ProductConversation aggregate identity.
  - Preserve read-through merge/exclusion semantics across refresh/upsert/remove while using aggregate keys.
- `ios/PhoenixMobile/Sources/AppModel.swift`
  - Update coordinator/list/cache ownership only as needed to consume aggregate-keyed identities.
  - Do not migrate `ConversationSession` ownership, SSE routing, message persistence, or outbox durability beyond the minimum compatibility seam.
- Focused tests and contract validation under `ios/PhoenixMobile/Tests/`
  - REST/model compatibility tests for aggregate DTO decode.
  - List-store merge/upsert/remove tests using aggregate identity.
  - Any focused mock-server or fixture-contract validation needed to prove the new wire contract.

## Explicit non-scope

B1 does **not** own:

- aggregate SSE or aggregate outbox/message-store redesign
- `ConversationSession` delegation/composition beyond the seam needed so B2 can consume the new shared models
- delete, follow-up/source retrieval, grounding/files, comments, or prose-reader behavior
- server changes
- broad UI rewrites outside the minimum list/coordinator compatibility surface

## Required shared seam for B2

Publish a deliberately small committed seam that downstream implementation can rely on before B2 edits:

- one canonical ProductConversation model family in `API/Models.swift`
- one canonical API route/response surface in `API/PhoenixAPI.swift`
- one aggregate identity accessor used by `ConversationListStore` and `AppModel`
- clear consumer guidance in the PR/task notes naming the exact symbols/files B2 must use for:
  - user-facing aggregate identity
  - aggregate list rows / coordinator row handling
  - transcript-row ids retained for `ConversationSession` and per-conversation durable stores

The seam should make it structurally obvious which id domain is aggregate-facing and which remains transcript-facing.

## Umbrella 04004 wording/status transition owned by B1

B1 also owns the exact umbrella wording correction if still needed: `04004` must explicitly state that permanent delete and History-stack work are **not prerequisites** for core ProductConversation migration. Do not leave umbrella wording that implies those are blocking requirements while implementing a narrower seam. Make that wording/status transition explicit in the B1 branch/PR, not implicit through code drift.

## Acceptance

- Native shared API types decode the authoritative ProductConversation REST fixtures/contracts shipped by `04005`.
- The user-facing list/cache merge logic is keyed by durable aggregate identity rather than transcript-row id.
- Coordinator/list ownership remains compatible with the new aggregate model.
- Focused unit tests cover aggregate DTO compatibility and aggregate-keyed list-store behavior.
- The resulting seam is small, committed, and safe for B2 to consume without guessing symbol ownership.
- Any umbrella `04004` wording that incorrectly implies permanent delete or History-stack prerequisites is explicitly corrected as part of this leaf.

## Validation / review expectations

Run the full applicable iOS checks plus focused validation for this seam:

- `cd ios/PhoenixMobile && xcodegen generate`
- `xcodebuild test -project PhoenixMobile.xcodeproj -scheme PhoenixMobile -destination 'platform=iOS Simulator,name=iPhone 16'`
- focused mock-server and/or fixture-contract validation for ProductConversation REST decoding if the branch adds or updates that harness

Before handoff/merge request:

- declare file ownership in the branch notes before editing
- obtain adversarial review and one independent review
- open a separate exact-HEAD PR for CI/review
- resolve all review threads
- do not merge or deploy from this task
