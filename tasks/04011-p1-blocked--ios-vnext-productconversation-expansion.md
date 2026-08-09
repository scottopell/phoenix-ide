# iOS vNext — ProductConversation migration and expansion

## Outcome

Coordinate the native-client migration and its systematic UI expansion without implementing against the superseded transcript-row conversation model.

## Gate

The program remains blocked until ProductConversation History/delete and the stable client-facing aggregate contract are on main.

## Ordered sections

1. ProductConversation migration.
2. Deterministic rendering fixture harness.
3. Conversation status and actions.
4. Tool-output rendering.
5. Markdown rendering.
6. Grounding and file browsing.
7. Prose reader and comments.

Section tasks are umbrella owners, not an instruction to implement a broad feature in one PR. Before a section becomes ready, split it into numbered, narrowly scoped taskmd leaf tasks with owning REQ IDs, explicit dependencies, observable acceptance, and fixture/test evidence.

## Completion

The umbrella completes only when the migration is shipped and every section's committed leaf queue is complete or deliberately removed from scope through a recorded product decision.

## Out of scope

ProductConversation architecture decisions themselves. Those remain in the ProductConversation program, specs, Allium, and ADR chain.
