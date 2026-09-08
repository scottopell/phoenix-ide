# iOS ProductConversation session delegation and transcript composition

Parent umbrella: task 04004.

## Why this leaf exists

Worker B2 owns the native session/composition slice after the deterministic fixture seam from task 04005 and after B1 lands the shared ProductConversation REST/wire/model seam. The current iOS client is still keyed to one transcript-row conversation identity end-to-end (`Conversation`, `ConversationSession`, `ConversationListStore`, `ConversationView`). This leaf scopes the aggregate-specific behavior without inventing parallel DTOs or identity types.

## Preconditions

- HEAD remains based on `13a3b75a9eb0ef16d30c1a2079203f82eabbabe0` before implementation starts.
- Task 04005 remains the authority for fixture contracts and read-only fixture stance.
- Implementation remains blocked even after this task is approved until B1 reports a committed seam SHA and the exact shared ProductConversation symbols B2 must consume.
- B1 first commits and reports the exact shared-contract seam for ProductConversation API/wire/model types.
- B2 consumes B1's shipped seam only; no parallel ProductConversation DTOs, member identity wrappers, or duplicated wire parsing.

## Scope

- Delegate `ConversationSession` runtime/SSE/chat/action ownership to the latest writable transcript member of one ProductConversation.
- Rebind deterministically when the aggregate's latest member or latest writable member changes.
- Compose one chronological aggregate transcript from member transcripts while preserving existing ordinary-session ownership of live SSE, cache, reducer, and outbox behavior.
- Enforce ProductConversation Open versus History behavior in the native detail surface:
  - Open: writable/latest-member delegation allowed when the aggregate exposes a writable member.
  - History: transcript remains readable while chat and Open-only state-transition actions are disabled.
- Integrate fixture-authority read-only behavior from 04005 without expanding into delete/follow-up/grounding/comments.
- Add focused tests for session rebinding, transcript composition, read-only behavior, simulator journeys, and mock-server coverage.

## Ownership map and file boundaries

B1-exclusive ownership for this umbrella slice:

- `ios/PhoenixMobile/Sources/API/Models.swift`
- `ios/PhoenixMobile/Sources/API/PhoenixAPI.swift`
- `ios/PhoenixMobile/Sources/Store/ConversationListStore.swift`
- `ios/PhoenixMobile/Sources/AppModel.swift` for aggregate list/cache/coordinator identity
- `ios/PhoenixMobile/Sources/Views/ConversationListView.swift` for canonical aggregate list navigation
- `ios/PhoenixMobile/Tests/ModelCompatibilityTests.swift`
- `ios/PhoenixMobile/Tests/ConversationListStoreTests.swift`

B2 implementation ownership after B1's seam lands:

- `ios/PhoenixMobile/Sources/Store/ConversationSession.swift`
- `ios/PhoenixMobile/Sources/Views/ConversationView.swift`
- narrowly new session/composition helper files if structurally needed
- `ios/PhoenixMobile/Tests/ConversationSessionReducerTests.swift`
- focused new session/composition tests under `ios/PhoenixMobile/Tests/`
- narrowly relevant fixture/UI journey files under `ios/PhoenixMobile/UITests/` and fixture support already authorized by 04005

B2 may read and consume B1-owned files to integrate the shipped seam. After B1 merged/retired, narrowly coordinated post-B1 edits are allowed only when specifically authorized for this task and kept within the approved constraints below.

`ios/PhoenixMobile/Tests/SSEParserTests.swift` is B2-owned only if the existing transcript-row SSE parser needs regression coverage for delegation/composition behavior. It is not a place to introduce aggregate SSE or alter wire parsing semantics.

## Coordinator-approved post-B1 integration handoff

B1 is merged/retired and no concurrent owner remains. For this task, B2 may make the following narrow post-B1 integration edits:

- `ios/PhoenixMobile/Sources/API/PhoenixAPI.swift` only for configuration identity used to fence transport handles.
- `ios/PhoenixMobile/Sources/AppModel.swift` only for ProductConversation detail/session composition, instance-owned Coordinator/cache/outbox dependencies, startup drain, reconfiguration invalidation, and sign-out-owned local reset. Standalone user-facing cache reset is out of scope for this task/release.
- `ios/PhoenixMobile/Sources/Store/ConversationListStore.swift` only to inject the existing cache-availability capability without changing aggregate identity/mapping semantics.
- `ios/PhoenixMobile/Sources/Views/ConversationListView.swift` only to enter the merged aggregate detail route while preserving B1 resolver authority.
- Matching seam tests covering these edits are in scope.

Still prohibited within this handoff:

- changing ProductConversation DTO or wire shapes;
- changing list identity semantics or aggregate identity/mapping semantics;
- introducing alternate caches or resolvers;
- changing B1 API endpoint contracts.

## Acceptance

- One ProductConversation opens as one native detail surface and delegates SSE/runtime actions to exactly one latest writable transcript member when available.
- Aggregate transcript composition is chronological across member transcripts with no duplicated durable messages.
- Rebind is deterministic when aggregate latest or writable member changes.
- History mode stays readable and disables chat plus Open-only state-transition actions.
- No aggregate SSE store, no aggregate outbox, no duplicated durable message cache.
- B2 consumes only B1's shared ProductConversation seam. Formerly B1-owned list/cache/API-model/navigation files may be touched only within the Coordinator-approved post-B1 integration handoff below; no ProductConversation DTO/wire-shape changes, list identity-semantic changes, alternate caches/resolvers, or B1 API endpoint-contract changes are allowed.
- Focused simulator/mock-server validation and applicable iOS checks pass.
- Standalone user-facing cache reset is out of scope; sign-out owns the total local reset path for server/account state.
- Adversarial review, independent review, separate PR exact-head CI/review, and zero unresolved threads are completed before merge request handoff.

## Out of scope

- Server changes.
- Aggregate-level SSE, outbox, or durable message storage.
- Delete, follow-up/source retrieval, grounding/files, prose/comments, or renderer expansion beyond what 04005 already established.
- Defining alternate ProductConversation models or identities beside B1's seam.
