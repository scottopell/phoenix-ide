# Recover in-app from provider prompt rejection

## Problem

A production Work conversation (`nvml-event-set-lifecycle-bug-fix`) was stranded after OpenAI rejected an automatic post-tool continuation with:

```text
invalid_prompt: Invalid prompt: your prompt was flagged as potentially violating our usage policy. Please try again with a different prompt
```

The tool call immediately before the rejection had succeeded, the existing worktree and uncommitted changes remained intact, and the provider explicitly instructed the user to try a different prompt. Phoenix nevertheless persisted the failure as `ErrorKind::InvalidRequest`, whose policy is non-auto-retryable and non-user-resumable. The conversation UI therefore hid both the composer and the retry action and displayed `Start a new conversation to continue.`

That presentation is semantically wrong for a prompt rejection that can be corrected by adding a revised user message. It also exposes an existing policy contradiction:

- `classify_responses_error` maps the explicit provider code `invalid_prompt` through the generic `lower.contains("invalid")` fallback to `LlmErrorKind::InvalidRequest`.
- `InvalidRequest` is intentionally non-user-resumable because many genuine malformed requests cannot be corrected by conversation text.
- the UI obeys that policy and hides all in-app recovery.
- `check_user_message_acceptable` and the core transition accept `UserMessage` from every `ConvState::Error`, including error kinds declared non-resumable.

The user can currently recover only by bypassing the UI and posting through the chat API/CLI or by creating a separate Direct conversation in the existing worktree. A long-running conversation should not appear lost when its files and execution environment are still healthy.

## Desired behavior

Represent provider prompt rejection as its own typed error condition, distinct from a structurally invalid API request.

The dedicated condition should have these exhaustive policies:

- automatic runtime retry: **no** — blindly replaying the identical provider request is likely to reproduce the rejection;
- user resume: **yes** — a new user turn changes the prompt and is the recovery the provider recommends;
- presentation: explain that the provider rejected the accumulated prompt and invite the user to revise it;
- in-app actions: keep the normal composer available and offer an explicit retry/continue action through the normal chat path.

The quick retry may send the existing `continue` message, but it must be described and implemented as a new user turn rather than an automatic replay of the failed provider request. The composer is required so the user can supply a more explicit benign/defensive framing when `continue` alone would not be sufficient.

## Correct-by-construction scope

### Provider classification

- Add a dedicated `LlmErrorKind` / persisted `ErrorKind` variant such as `PromptRejected` (name to be chosen consistently with provider-neutral semantics).
- Classify explicit provider codes such as OpenAI `invalid_prompt` before the generic invalid-request fallback.
- Do not make all `InvalidRequest` values resumable. Malformed payloads, unsupported parameters, and other request-shape failures must retain an explicit policy.
- Make an explicit, tested decision for equivalent prompt/content-filter codes from each provider rather than relying on substring coincidence.

### Runtime and wire contract

- Project the new kind through `llm_error_to_outcome`, persisted conversation state, generated TypeScript, SSE init/state-change payloads, and `ErrorPresentation` without collapsing it back into generic `RequestRejected` semantics that lose the recovery distinction.
- Ensure automatic retry and user-resume policy remain separate exhaustive functions.
- Align chat admission with the declared user-resume policy. A state must not simultaneously advertise `can_user_resume = false` while accepting a normal user message through the API.
- Prefer a structural distinction or typed transition input that prevents resumable and terminal error handling from drifting between state machine, API preflight, and UI.

### UI recovery

- Render tailored prompt-rejection copy rather than the generic `Error` / `Start a new conversation` dead end.
- Show the normal composer so the user can rewrite the next message.
- Offer the in-app retry/continue affordance and send it through the normal durable chat path.
- Preserve the conversation, task/work mode, worktree, pending diff, and transcript; recovery must not create a second worktree or require a production restart.
- Do not display or claim to identify a specific offending substring when the provider did not return one and Phoenix did not retain the assembled payload.

### Specifications

- Extend `specs/llm/` so prompt rejection is explicitly non-auto-retryable but user-resumable by changed user input.
- Extend `specs/conversation-ui/` / `specs/bedrock/` so error-state compose and chat admission derive from the same typed user-resume capability.
- Reconcile existing spec drift: `REQ-CONV-021` names usage-limit exhaustion as non-resumable even though `REQ-LLM-006`, the LLM Allium model, and current implementation classify a resettable usage-limit window as user-resumable.
- Follow `specs/AUTHORING.md` before pushing any spec change.

## Acceptance criteria

- [ ] OpenAI Responses error code `invalid_prompt` maps to a dedicated provider-neutral prompt-rejection kind, not `InvalidRequest` via substring fallback.
- [ ] Prompt rejection is exhaustively `NoAutoRetry` and `UserResumable` in both LLM-domain and persisted error policy.
- [ ] A prompt rejection does not trigger an automatic replay of the same request.
- [ ] A persisted prompt-rejection state exposes `can_user_resume: true` over init and live SSE.
- [ ] The error banner explains that the accumulated prompt was rejected and that a revised message can continue the same conversation.
- [ ] The composer remains available in that error state and can submit revised text.
- [ ] The in-app retry/continue action submits a new durable user message and transitions the existing conversation back to `LlmRequesting`.
- [ ] Work-mode identity, worktree attachment, transcript, and uncommitted files remain unchanged across recovery.
- [ ] Genuine non-resumable request-shape failures do not expose the composer or accept chat messages through a hidden API path.
- [ ] Backend tests exhaustively cover every error kind's auto-retry and user-resume policies, including prompt rejection, invalid request, content filter, context exhaustion, auth, overload, and usage-limit reset.
- [ ] Provider-classifier tests cover explicit `invalid_prompt` routing and prove generic invalid-request routing remains distinct.
- [ ] State-machine/API tests prove UI policy and chat admission cannot disagree for resumable versus terminal error states.
- [ ] UI tests cover prompt-rejection copy, visible composer, quick retry, revised-message submission, and the terminal invalid-request counterexample.
- [ ] Normative LLM, conversation UI, and bedrock specs agree on the policy matrix and pass their validation.
- [ ] `./dev.py codegen` is run if generated wire types change, and `./dev.py check` passes.

## Validation journey

1. Put a conversation with an attached Work scope into the typed prompt-rejection error state after a successful tool result.
2. Reload/reconnect and verify the banner and composer reconstruct from persisted state.
3. Submit a revised benign/defensive message in-app.
4. Verify the same conversation and Work scope transition to requesting, the message persists once, and the existing worktree/diff remain intact.
5. Exercise a true malformed-request `InvalidRequest` and verify it remains terminal in both the UI and chat API.
6. Run provider classification tests, state-machine/API tests, focused UI tests, spec validation, and the full Phoenix check.

## Non-goals

- Bypassing or weakening provider safeguards.
- Persisting complete provider payloads, moderation classifier output, or inferred offending substrings.
- Automatically retrying an unchanged rejected request.
- Treating all invalid requests or all provider refusals as resumable without an explicit policy decision.
- Changing the conversation's model automatically.
