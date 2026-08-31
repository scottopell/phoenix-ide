# iOS vNext — ProductConversation migration and expansion

## Outcome

Coordinate the native-client migration and its systematic UI expansion without implementing against the superseded transcript-row conversation model.

## Gate

The deterministic fixture harness proceeds independently. Core migration may begin after its fixture contract seam is committed and reviewed, consuming the shipped ProductConversation REST aggregate while preserving transcript-row SSE/session ownership. Permanent delete and later capability sections do not block core migration.

## Ordered sections

1. Deterministic rendering fixture harness.
2. ProductConversation migration.
3. Conversation status and actions.
4. Tool-output rendering.
5. Markdown rendering.
6. Grounding and file browsing.
7. Prose reader and comments.

The fixture harness is one bounded implementation task. Other section tasks are umbrella owners, not instructions to implement broad features in one PR; split them only when scheduled concurrent work has genuinely separate source ownership.

## Completion

The umbrella completes only when the migration is shipped and every section's committed leaf queue is complete or deliberately removed from scope through a recorded product decision.

## Out of scope

ProductConversation architecture decisions themselves. Those remain in the ProductConversation program, specs, Allium, and ADR chain.
