# Make direct-turn LLM outcomes durable across local persistence failure

Migrate direct chat and ordinary LLM execution onto the merged durable-workflow foundation so a completed provider response cannot be forgotten or silently reissued after `SQLITE_BUSY`, process loss, or runtime recreation.

## Required vertical slice

- Durably accept direct turns with stable client identity and `Committed` / `Replay` / `Conflict` outcomes.
- Persist immutable prepared LLM payloads and durable effect identity before provider dispatch.
- Persist provider completion as a canonical durable response receipt before product-state acceptance.
- Atomically accept the response into assistant message/checkpoint, next conversation state, runtime acceptance, delivery disposition, and normalized durable tool intents.
- Broadcast and dispatch tools only after commit.
- Automatically create another attempt for a top-level LLM effect whose provider outcome was lost in a hard crash; never reissue an effect with a durable completed-response receipt. This provides at-most-one product-accepted response while permitting at-least-once provider submission after ambiguous process failure.

## Decisive regression

Use real SQLite files and separate connections. Cover temporary receipt-persistence lock contention, crash before terminal receipt, crash after receipt but before product acceptance, crash after acceptance but before tool dispatch, crash after tool execution may have begun, both Stop transaction orderings, partial-stream replacement, and representative provider adapters. Verify that an unknown attempt is automatically resubmitted, a durable completed-response receipt is never resubmitted, only one response becomes product-authoritative, and tool dispatch occurs only from committed durable intent.

Read and extend the merged durable-workflow requirements and direct-chat Allium profile before editing provider loops. Do not treat last durable conversation state as safe when it predates a completed external effect.
