# Split error retry policy from user resume capability

## Problem

A production conversation (`replace-custom-diff-viewer-integration-3`) hit an auth failure from the Codex/OpenAI bridge:

```text
Authentication failed: {"errors":[{"status":"401","title":"Unauthorized","detail":"Invalid internal auth token: ('Invalid token error', ExpiredSignatureError('Signature has expired'))"}]}
```

The UI then rendered:

```text
Start a new conversation to continue.
```

instead of offering `Retry — sends "continue"`.

This is a correct-by-construction violation: the code uses one semantic flag/word (`retryable`) for at least two distinct capabilities:

1. **automatic retry safety** — whether the runtime may retry without user action/backoff while the same turn is in flight.
2. **user resume capability** — whether a persisted error state may accept a user-triggered resume message after the user fixes the condition.

Auth errors are generally not auto-retryable, because immediate automatic retries can loop until credentials are fixed. But auth errors are user-resumable: after refreshing tokens, re-running login, fixing env config, or allowing the credential helper to complete, the user should be able to resume the same conversation by sending `continue`.

The current implementation collapses these concepts, so the UI hides the resume affordance for `auth` even though the state machine and chat preflight already allow `ConvState::Error + UserMessage`.

## Evidence from code

- `crates/phoenix-ide/src/llm/error.rs`
  - `LlmErrorKind::is_retryable()` means auto retry and returns `false` for `Auth`.
- `crates/phoenix-ide/src/db/schema.rs`
  - `ErrorKind::is_retryable()` mirrors that same auto-retry set and returns `false` for `Auth`.
- `crates/phoenix-ide/src/state_machine/transition.rs`
  - `CoreState::Idle | CoreState::Error` accepts `CoreEvent::UserMessage` and enters `LlmRequesting { attempt: 1 }`.
  - `check_user_message_acceptable()` also returns `Ok(())` for `ConvState::Error { .. }`.
- `crates/phoenix-ide/src/llm/service.rs`
  - streaming auth failures invalidate cached credentials but intentionally do not auto-retry the stream; the next request should use fresh credentials.
- `ui/src/components/ErrorBanner.tsx`
  - hand-maintains a string list based on backend `ErrorKind::is_retryable()` and hides retry unless `error_kind` is one of `rate_limit`, `network`, `server_error`, or `timed_out`.
  - This frontend list is a parallel representation of backend policy and can drift.
- `ui/src/api.ts` / `ui/src/utils.ts`
  - conversation error state exposes `error_kind?: string`, so the UI has no generated/exhaustive type forcing every backend `ErrorKind` to map to UI affordances.
- `crates/phoenix-ide/src/runtime/user_facing_error.rs`
  - `UserFacingErrorKind::{Retryable,Fatal,Internal}` has the same naming smell for transient SSE/API errors: `Retryable` currently mixes “try this operation again” with broader recoverability, while `Fatal` says auth/config needs external action even though it may be resumable after that action.

## Correct-by-construction fix

Introduce structurally separate error-policy concepts and make consumers choose the correct one by type.

### Backend

1. Replace or supplement ambiguous `is_retryable()` with explicit capability methods/types:

```rust
pub enum AutoRetryPolicy {
    AutoRetryable,
    NoAutoRetry,
}

pub enum UserResumePolicy {
    Resumable,
    NotResumable,
}
```

or equivalent strongly typed values.

2. For `LlmErrorKind` and persisted `ErrorKind`, define both policies exhaustively:

- auto-retryable:
  - `Network`
  - `RateLimit`
  - `ServerError`
  - `TimedOut` where applicable
- user-resumable:
  - `Auth`
  - `Network`
  - `RateLimit`
  - `ServerError`
  - `ServerOverloaded` if changing model then continuing is supported/desired
  - `TimedOut`
  - possibly `InvalidRequest` only if a new user message can correct it; otherwise leave non-resumable
- not user-resumable:
  - `UsageLimitReached` unless switching model then continuing is explicitly supported
  - `ContextExhausted` (existing handoff/continue flow is separate)
  - `ContentFilter`
  - `Cancelled` if the state should be dismissed/idle rather than resumed
  - `SubAgentError` for parent-facing banner unless a concrete resume path exists

3. Rename call sites so incorrect use is mechanically obvious:

- state-machine automatic retry guards should use `auto_retry_policy()` / `is_auto_retryable()`.
- UI/API presentation should use `user_resume_policy()` / `is_user_resumable()`.
- Avoid leaving a generic `is_retryable()` method that future code can misuse.

4. Export a generated TypeScript representation for the persisted error kind and/or its policy.

   Preferred: add a typed wire struct for error state presentation, e.g.

```rust
pub struct ErrorPresentation {
    pub kind: ErrorKind,
    pub can_auto_retry: bool,
    pub can_user_resume: bool,
    pub recovery_hint: ErrorRecoveryHint,
}
```

   The UI should receive this directly or derive it from a generated exhaustive TS map, not from ad hoc string arrays.

5. Update `UserFacingErrorKind` naming if it participates in retry affordances. At minimum, avoid using `Retryable/Fatal` to decide conversation-resume UI. Prefer explicit names such as:

```rust
pub enum UserFacingErrorAction {
    RetryOperation,
    UserActionThenRetry,
    StartNewConversation,
    None,
}
```

or keep it scoped/documented as transient SSE operation retry only, with tests preventing it from being used for persisted conversation error banners.

### Frontend

1. Remove the hand-maintained retryable string list from `ErrorBanner.tsx`.
2. Render the retry button from typed `canUserResume` / `user_resume_policy` instead.
3. For auth errors, show copy that reflects the actual recovery path, e.g.

```text
Authentication failed. Refresh or fix auth, then retry — sends “continue”.
```

4. Preserve special usage-limit copy, but make that a typed recovery hint/policy rather than an override that competes with retryability.
5. Replace `error_kind?: string` with generated/validated typed error-kind data where practical. If the `ConvState` JSON remains `unknown` in generated SSE state, add a local schema/decoder that validates known `error_kind` values exhaustively and fails loudly in tests when backend variants change.

### Specs

Update the LLM and/or conversation UI specs so the semantic distinction is normative:

- LLM spec: retryable means **automatic retry by the runtime**, not user resume.
- Conversation UI / bedrock behavior: persisted `Error` states that accept `UserMessage` must expose a user-resume affordance unless the specific error kind is structurally non-resumable.
- Credential-helper/auth spec: auth failures are non-auto-retryable but user-resumable after credentials become available/fixed.

If an Allium spec already models `retryable` on `LlmError`, add a separate derived field for `user_resumable` or keep that concept in the conversation/UI spec rather than overloading the LLM provider spec.

## Acceptance criteria

- [ ] There is no ambiguous `is_retryable()` API used for both runtime auto retry and UI resume affordances.
- [ ] Runtime automatic retry behavior remains unchanged for auth: auth does **not** auto-retry in a tight/backoff loop.
- [ ] A persisted `ConvState::Error { error_kind: Auth, .. }` renders a resume/retry affordance in the UI.
- [ ] Clicking that affordance sends `continue` through the existing chat path and is accepted from `ConvState::Error`.
- [ ] The frontend no longer contains a hand-maintained string list mirroring backend retry policy.
- [ ] Backend tests exhaustively assert auto-retry and user-resume policy for every `LlmErrorKind`/`ErrorKind` variant.
- [ ] UI tests cover at least:
  - auth error shows retry/continue affordance
  - usage-limit or context-exhausted style non-resumable error does not show the same affordance
  - retryable transient errors still show retry
- [ ] Specs are updated so future changes cannot reintroduce the conflation.
- [ ] `./dev.py codegen` is run if any generated TS types change.
- [ ] `./dev.py check` passes.

## Suggested implementation notes

Do not solve this by adding `'auth'` to the `ErrorBanner.tsx` array. That would fix the symptom while preserving the structural bug: a UI string list would still be pretending to know backend error policy.

The target design should make the wrong question hard to ask. Code that wants to decide automatic retries should only have access to an auto-retry policy. Code that wants to decide whether a user can resume should only have access to a user-resume policy/action.
