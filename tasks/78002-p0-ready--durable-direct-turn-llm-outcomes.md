# Make direct-turn LLM outcomes durable across local persistence failure

Migrate direct chat and ordinary LLM execution onto the merged durable-workflow foundation so a completed provider response cannot be forgotten or silently reissued after `SQLITE_BUSY`, process loss, or runtime recreation.

## Required vertical slice

- Durably accept direct turns with stable client identity and `Committed` / `Replay` / `Conflict` outcomes.
- Persist immutable prepared LLM payloads and durable effect identity before provider dispatch.
- Persist provider completion as a canonical durable response receipt before product-state acceptance.
- Atomically accept the response into assistant message/checkpoint, next conversation state, runtime acceptance, delivery disposition, and normalized durable tool intents.
- Broadcast and dispatch tools only after commit.
- Represent ambiguous non-repeatable outcomes explicitly rather than blindly retrying.

## Decisive regression

Use a real SQLite file and separate connections. Hold a writer lock after a mock provider returns a successful response containing a tool call, force `SQLITE_BUSY`, and verify one provider call and no tool dispatch. Release the lock, recreate the runtime/process boundary, accept the same durable response without another provider call, and verify exactly one assistant message and one durable tool intent. Repeat for a non-idempotent tool result.

Read and extend the merged durable-workflow requirements and direct-chat Allium profile before editing provider loops. Do not treat last durable conversation state as safe when it predates a completed external effect.
