# Release direct-turn ownership after final LLM outcomes

Production Coordinator sends return `conversation_busy` after a successful turn reaches Idle. The direct-turn aggregate remains Active with `owns_conversation = 1` because terminal classification runs for reducer events but not final LLM outcomes processed through `handle_outcome`.

## Acceptance criteria

- Final LLM outcomes that settle an authoritative direct turn terminalize the durable turn after state persistence.
- A focused regression proves normal text completion releases ownership immediately.
- Existing startup reconciliation still recovers an already-materialized stuck turn after restart.
- Production is recovered through the normal deploy/restart path, not direct database mutation.
