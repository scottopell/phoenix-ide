# Idempotent targeted conversation cancellation

Anonymous POST /cancel cannot distinguish a lost-response retry from a fresh request to cancel a successor turn. Introduce explicit operation and target identities without coupling ordinary execution cancellation to ProductConversation Close.

## Acceptance criteria

- [ ] Accept a client-generated cancellation operation ID.
- [ ] Target an exact execution/direct-turn identity.
- [ ] Same operation and target replay converges to the durable outcome.
- [ ] Reusing an operation ID for another target returns a typed conflict.
- [ ] Persist a durable cancellation receipt and outcome.
- [ ] A lost-response retry cannot cancel a successor turn.
- [ ] ProductConversation Close uses its root-scoped Close attempt to call typed settlement operations rather than anonymous cancellation.
- [ ] SSE and UI project the committed outcome.
- [ ] Migration and client compatibility are explicit and tested.
