# Cut over tool effect claims, results, and recovery

Depends on authoritative repository and LLM effect cutover.

Scope:
- durable tool intents as sole execution authority
- independently claimable non-empty tool effect sets
- tool completion/recovery/cancellation races
- deletion of provider-response-derived execution authority

Acceptance:
- Runtime tool calls derive only from authoritative tool intents.
- Each tool effect has one typed lifecycle and claim authority.
- Cancel/result/recovery interleavings have deterministic single winners.

Out of scope: steering and UI/SSE projection cutovers.
