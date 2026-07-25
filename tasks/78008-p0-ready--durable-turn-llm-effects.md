# Cut over LLM effect claims, results, and recovery

Depends on authoritative durable-turn repository and acceptance cutover.

Scope:
- prepared LLM requests, effect/attempt claims, result/failure observations
- recovery workers and metrics identity
- elimination of timestamp-inferred ownership and sleep-based correctness

Acceptance:
- Owed -> Claimed -> Accepted/Released/Interrupted is authoritative and typed.
- Recovery dispatches only authoritative owed/interrupted effects.
- Terminal/cancel fences stale results by generation.
- No sleeps or polling provide correctness.

Out of scope: tool and steering cutovers.
