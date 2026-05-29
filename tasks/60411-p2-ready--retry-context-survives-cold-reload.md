Retry context (attempt N/max, reason, resets_at) disappears from the StateBar on a FRESH page reload that lands mid-retry-backoff.

## Symptom
A turn is on attempt 3/3 during the LLM retry backoff window. The user hard-reloads the page (new EventSource, no Last-Event-ID, snapshot-only Init — NOT a reconnect). The StateBar shows a bare "awaiting LLM response Ns" with no "(retry 3/3 after rate limit)" suffix. Before PR #155, getStateDescription derived "thinking (retry N)..." from the persisted ConvState.attempt, so the count at least survived reload.

## Why it happens
turnRetryContext is populated ONLY by the transient sse_llm_attempt event and is never seeded on Init (ui/src/conversation/atom.ts createInitialAtom + the sse_init reducer set it to null). A mid-backoff RECONNECT is fine — llm_attempt is in the replay-ring whitelist (specs/sse_wire/sse_wire.allium) so it replays. But a cold reload gets the Init snapshot only, which carries ConvState.attempt but not the retry reason/max/resets_at.

## Why not fixed in the PR #155 review pass
ConvState::LlmRequesting carries only attempt — not reason, max_attempts, or resets_at. Those live solely on the ephemeral LlmAttempt wire event. A correct fix needs one of:
  (a) Server persists the full retry context (reason + resets_at + max) on the conversation row or assistant message, and Init carries it; or
  (b) Init synthesizes a partial turnRetryContext from ConvState.attempt + MAX_RETRY_ATTEMPTS, showing "(retry N/3)" without the reason (degraded but better than nothing); or
  (c) Server re-emits LlmAttempt into the Init/snapshot path for an in-backoff turn.

Recommend (a) for full fidelity; (b) is a cheap interim. Decide and implement.

## Scope
- Backend: persist or snapshot-carry retry reason/max/resets_at (specs/llm-retry-visibility/ producer contract).
- Frontend: seed turnRetryContext in the sse_init reducer (ui/src/conversation/atom.ts).
- Spec: update specs/working-phase-visibility/ REQ-WPV-003 + specs/llm-retry-visibility/ for the cold-reload path.

## Source
Code review of PR #155 (finding #1). Narrow window (cold reload during the short backoff), but a real regression vs pre-PR behavior.
