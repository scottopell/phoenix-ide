# ADR-055: Invalid-request errors allow manual recovery

- **Status:** Accepted
- **Date:** 2026-09-18
- **Affects:** REQ-LLM-006; LlmError user-resume partition; bedrock user_resumable helper

## Context

A production Global Coordinator receives a Codex 400 stating that `access_programs` is not enabled for the organization, despite Phoenix not emitting that parameter. The request is classified as InvalidRequest. The same classification disables both automatic retry and manual recovery, leaving the conversation unusable in the running process. Startup normally clears this state, but restarting the server is an unnecessarily broad recovery action.

A 400 cannot prove whether the cause is permanently malformed input, a configuration problem the user can fix, or a transient provider routing or entitlement fault. The existing separate automatic-retry and user-resume policies can express this distinction without introducing a provider-message heuristic.

## Options considered

1. **Special-case the access-program message as a transient server error** — permits automatic retry, but guesses at an unverified cause and depends on unstable provider wording.
2. **Keep invalid requests non-resumable** — avoids repeated rejected requests, but forces the user to restart Phoenix or abandon the conversation even after the cause clears.
3. **Keep automatic retry disabled and permit manual recovery for InvalidRequest** — lets the user decide when to retry or revise the request while preserving the existing conversation.

## Decision

Choose option 3. Both provider and persisted conversation error policies treat InvalidRequest as user-resumable and non-auto-retryable. Retry starts a fresh user-authorized turn through the existing message path; Dismiss returns to Idle without dispatching a model request. Completed tools are not replayed by either transition.

The policy also applies to existing persisted invalid-request states. It changes no stored encoding, adds no migration, and does not depend on the error message. The SSE projection and client-side presentation must agree. Content-filter, context-exhaustion, and lifecycle-terminal policies remain unchanged.

In-process runtime recreation preserves every persisted user-resumable error, including its diagnostic, reset time, and state-entry time. Model-change eviction must not reinterpret a completed tool tail as authority to retry before the user acts, or replace the error with Idle before a queued dismissal arrives. Full process startup retains its separate database-reset semantics.

Provider content-filter outcomes retain their category through the executor and state machine. Client-side state decoding failures use a distinct client-only state without a provider error kind or recovery presentation. The client offers reload guidance and withholds chat, model-change, retry, dismissal, and cancellation controls until it can read authoritative state. An unreadable state is not evidence of a resumable server-side request error.

## Consequences

- **Positive:** External provider problems cannot permanently strand a conversation solely because they were reported as an invalid request.
- **Positive:** Recovery uses existing conversation identity, transcript, and transition machinery.
- **Negative:** A manual retry of a genuinely malformed request can fail again; the provider's diagnostic remains visible for investigation.
- **Neutral:** This does not establish that the observed upstream fault is transient, add automatic replay, or alter startup recovery.

## References

- `specs/llm/requirements.md` — REQ-LLM-006
- `specs/llm/llm.allium` — UserResumablePartition
- `specs/bedrock/bedrock.allium` — user_resumable
- `LlmErrorKind::user_resume_policy`, `ErrorKind::user_resume_policy`
- `check_user_message_acceptable`, `ErrorPresentation::from_kind`
