# Stabilize Explore-mode task ID hint and prompt caching

## Problem

Explore-mode prompts currently include a precomputed “next available taskmd ID” because agents cannot run `taskmd next` in read-only mode. The prompt builder recomputes that ID from the live `tasks/` directory every LLM request.

That means a normal Explore workflow can invalidate itself:

1. The first LLM request sees `43002` and drafts `tasks/43002-p2-ready--...md`.
2. The tool result triggers another LLM request before `propose_task`.
3. The system prompt is rebuilt, sees the new file, and now says the next ID is `43003`.
4. The agent may think it used the wrong ID and rewrite/chase the moving target.

Because the full system prompt is sent as `SystemContent::cached(...)` with a stable conversation cache key, this also changes cached prompt bytes mid-conversation and likely degrades prompt-cache hit rates.

## Evidence

Relevant code paths:

- `crates/phoenix-ide/src/system_prompt.rs`
  - `next_taskmd_id(...)` computes `taskmd_core::ids::next_id(&tasks_dir)` from disk.
  - `build_system_prompt_with_options(...)` calls it every time `ModeContext::Explore` is rendered.
- `crates/phoenix-ide/src/runtime/executor.rs`
  - The executor rebuilds `build_system_prompt(...)` for each LLM request.
  - The resulting prompt is wrapped in `SystemContent::cached(&system_prompt)` and paired with `PromptCacheKey::stable(&conv_id)`.

## Proposed fix

Make the Explore task ID hint stable for the lifetime of the proposal workflow/conversation instead of recomputing it from disk every LLM loop.

Reasonable implementation options:

1. Snapshot the next taskmd ID when the Explore conversation/worktree is created and store it in conversation mode metadata or a small typed persisted field.
2. Thread that stored ID into `build_system_prompt(...)` so prompt construction is deterministic across turns.
3. Continue to omit the hint when the project does not use taskmd (`_TEMPLATE.md` marker absent).
4. If the snapshotted ID becomes unavailable due to an external concurrent task creation, handle the collision at `propose_task`/approval time with an explicit error or deterministic rename path—not by changing the system prompt underneath the agent.

## Test plan

Add tests at two levels:

1. **Prompt-builder unit test**
   - Build an Explore prompt for a temp taskmd repo and extract the hinted ID.
   - Create `tasks/{hinted-id}-p2-ready--draft.md`.
   - Build the prompt again using the same conversation/mode metadata.
   - Assert the hinted ID is unchanged and the newly computed filesystem ID does not appear.

2. **Mandatory executor/request-level cache-shape test**
   - Use a fake/mock `LlmService` that records each `LlmRequest` passed to `complete_streaming`.
   - Drive an Explore turn where the model first calls `patch` to create the hinted task file, causing a follow-up LLM request before `propose_task`.
   - Assert `request.system` is identical across both LLM requests and `request.cache_key` remains the same stable conversation key.
   - This directly protects prompt-cache economics and prevents the broader bug class: same cache key plus same system/tool/message prefix shape is the provider-facing property needed for cache reuse.

## Acceptance criteria

- In an Explore conversation, the injected “next available taskmd ID” remains unchanged after the agent writes a task file with that ID.
- The system prompt bytes remain stable across tool-result LLM loops unless guidance, skills, mode, or other intentional prompt inputs change.
- Add regression coverage for the sequence: build prompt → create task file with hinted ID → build prompt again → same hinted ID.
- Add or identify request-level coverage that asserts repeated LLM requests in the same Explore tool loop keep identical cached system content and the same stable cache key.
- Existing behavior remains unchanged for non-taskmd projects and for Work/Direct/Branch mode prompts.
