# Add typed auth recovery resume targets

## Problem

When a user manually ends/summarizes a conversation and the credential helper token has expired, the continuation-summary LLM request can fail with an auth error. Today that error is handled by the generic continuation failure path, which immediately transitions the conversation to `context_exhausted` and persists fallback text such as:

> Context limit reached. The continuation summary could not be generated: Waiting for authentication — complete the sign-in flow to continue. Please start a new conversation.

That text is then displayed, copied, and used to seed the next conversation as if it were a real summary. The user sees a terminal context-exhausted state with bogus summary content instead of a visible auth flow.

## Root cause

Phoenix already has a visible auth recovery state, `AwaitingRecovery`, but that state implicitly means “when credentials recover, resume the ordinary conversation LLM turn.” Continuation summary generation is also an LLM operation, but it cannot use that recovery path because the state does not structurally remember what operation was suspended.

As a result, `AwaitingContinuation + LlmError(Auth, recovery_in_progress = true)` falls through to the continuation failure behavior and persists auth text as a continuation summary.

## Design direction

Make auth recovery a wrapper around a typed, resumable LLM operation.

`AwaitingRecovery` must carry a typed resume target instead of assuming every recovery resumes `Effect::RequestLlm`.

Suggested shape:

```rust
enum RecoveryResumeTarget {
    ConversationTurn,
    ContinuationSummary(ContinuationSummaryRequest),
}

struct ContinuationSummaryRequest {
    rejected_tool_calls: Vec<ToolCall>,
}

AwaitingRecovery {
    message: String,
    error_kind: ErrorKind,
    recovery_kind: RecoveryKind,
    resume: RecoveryResumeTarget,
}
```

The exact names can differ, but the important invariant is structural: recovery success must know what semantic LLM operation to resume without inspecting error text, UI state, or other implicit context. The continuation request type owns its operation-specific inputs; recovery only carries the suspended operation.

## Desired behavior

1. If any recoverable LLM operation gets an auth error while credential recovery is in progress, Phoenix enters `AwaitingRecovery { resume: ... }`.
2. If the suspended operation was the ordinary conversation turn, credential success resumes `Effect::RequestLlm` as today.
3. If the suspended operation was continuation summary generation, credential success resumes `Effect::RequestContinuation` with the original `ContinuationSummaryRequest`.
4. Credential recovery failure surfaces an explicit auth/error state; it must not fabricate or persist a continuation summary.
5. Continuation fallback summaries remain valid only for genuine continuation-generation failures after normal retry policy, not for auth recovery states.

## Implementation plan

### State machine / core types

- Add a typed recovery resume target to the recovery state.
- Update construction of `AwaitingRecovery` from ordinary LLM auth failures to use `RecoveryResumeTarget::ConversationTurn`.
- Add central handling for auth errors during continuation generation to enter `AwaitingRecovery` with `RecoveryResumeTarget::ContinuationSummary(request)`.
- Update `CredentialBecameAvailable` handling to dispatch the effect implied by the typed resume target:
  - `ConversationTurn` → `Effect::RequestLlm`
  - `ContinuationSummary(request)` → `Effect::RequestContinuation(request)`
- Update `CredentialHelperFailed` handling so recovery failure becomes a visible auth error and never persists a continuation fallback summary.
- Preserve existing retry/fallback behavior for non-auth continuation errors.

### Runtime/executor

- Ensure the existing credential-settlement watcher continues to operate whenever the conversation is in `AwaitingRecovery`, regardless of resume target.
- Ensure the continuation resume path calls the existing continuation request implementation with the preserved `ContinuationSummaryRequest`.
- Keep `recovery_in_progress` propagation from LLM auth errors; the continuation request already sends this field in `Event::LlmError`.

### UI

- Reuse the existing `awaiting_recovery` rendering and `CredentialHelperPanel`; no continuation-specific auth UI should be necessary.
- Confirm that continuation auth recovery displays the auth panel instead of the context-exhausted banner.
- Optionally add a lightweight UI preflight for manual “End & summarize now”: if `credential_status` is `required`, `running`, or `failed`, open the auth panel before starting continuation. This is defense-in-depth only; correctness must live in the state machine.

### Specs/tests

- Update `specs/bedrock` so credential recovery applies to resumable LLM operations, not only ordinary `llm_requesting` turns.
- Add state-machine tests for:
  - ordinary LLM auth recovery still resumes the main turn.
  - continuation auth error with `recovery_in_progress = true` enters `AwaitingRecovery` with a continuation resume target.
  - credential success from continuation recovery retries continuation and preserves the `ContinuationSummaryRequest`.
  - credential failure from continuation recovery does not persist a fallback continuation summary.
  - non-auth continuation failure still produces a fallback summary after retries.
- Add UI/regression coverage if practical for the banner/auth-panel behavior.

## Acceptance criteria

- Reproducing the expired-token manual summarize flow opens/shows auth recovery instead of storing auth text as the summary.
- After completing auth, Phoenix generates and persists a real continuation summary.
- “Continue in new conversation” never seeds a new conversation with credential-helper/auth failure copy.
- Existing normal-turn auth recovery behavior is preserved.
- Existing context-exhaustion fallback behavior still works for genuine non-auth continuation failures.
