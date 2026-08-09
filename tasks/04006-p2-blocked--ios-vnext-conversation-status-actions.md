# Complete iOS vNext conversation status and actions

## Outcome

Represent ProductConversation lifecycle, live execution state, and available actions honestly in the native client.

## Dependencies

Blocked by ProductConversation migration and the rendering fixture harness.

## Scope

Inventory the shipped ProductConversation and execution-state contract, then create numbered leaf tasks for each missing or incorrect native state/action presentation. Include Open, History, closing/repair, working, blocking decisions, errors, cancellation, and read-only behavior.

Leaf tasks must cite the owning REQ IDs and prove behavior through focused tests plus deterministic fixtures.

## Acceptance

- Every shipped state has an intentional native presentation.
- Actions derive from server-owned capability/lifecycle truth.
- Offline, cached, stale, in-flight, and read-only states cannot expose invalid actions.
- The section has no untracked generic or invisible state fallbacks.

## Out of scope

Tool-result rendering and file/reader surfaces.
