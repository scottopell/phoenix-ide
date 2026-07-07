# Investigate continuation summary context rejection

## Problem

Production conversation `365820bb-9b0d-48cc-b256-7e462b13e7c1` (`commission-review-v2-partial-success-and-trustworthy-status-contract`) reached context exhaustion and attempted a continuation summary. Phoenix logged:

```text
continuation: trimmed history to fit budget and start user-first
  dropped=1
  trimmed_for_user_first=1
  context_window=272000
  input_budget=266918

Continuation LLM request failed:
  context_length_exceeded: Your input exceeds the context window of this model.
```

Known facts from prod DB/logs:

- model was `gpt-5.5`
- effective runtime context window logged by Phoenix was `272000`
- the conversation had `966` stored messages
- stored message content totaled about `2,060,132` bytes
- immediately before continuation, usage-bearing messages were near the continuation threshold; the latest recorded usage was `input_tokens=559`, `cache_read_tokens=244224`, `context_used=244783`
- continuation compaction logged that it dropped `1` message and trimmed `1` leading non-user message
- the provider rejected the resulting continuation request with `context_length_exceeded`

What is not yet known:

- the actual token count of the exact continuation request sent to the provider
- whether Phoenix's estimator undercounted text, message framing, images, system/prompt overhead, cached-prefix accounting, or something else
- whether rendered history for continuation differs materially from the prior provider-measured request because tool blocks are flattened
- whether `cache_read_tokens` should be interpreted as part of the provider context limit in this failure mode
- whether the Codex bridge applies additional request-shape overhead or a lower effective limit than Phoenix's `272000` cap

## Goal

Produce an evidence-backed root cause for this production failure and a narrowly justified fix plan. Do not implement adaptive retry, estimator changes, or truncation policy changes until the investigation explains which safeguard failed and why.

## Investigation plan

### 1. Reconstruct the exact continuation request shape

Using the prod DB row for `365820bb-9b0d-48cc-b256-7e462b13e7c1`, reproduce the same transformation pipeline used by `request_continuation`:

1. render DB messages into `LlmMessage`s
2. `flatten_tool_blocks`
3. `cap_replayed_images`
4. compute fixed-token reserve from continuation prompt, system prompt, output reserve, and safety margin
5. `cap_messages_to_token_budget`
6. `drop_leading_non_user`
7. append the continuation request

Record:

- number of messages before/after each step
- estimated tokens before/after each step
- content block counts by kind
- total chars/bytes by role and by original message type where available
- whether any single retained message or block dominates the request

### 2. Compare estimator output with provider-observed usage

For nearby successful turns, compare Phoenix's local estimate against stored `usage_data`:

- local estimate for the same rendered messages
- provider `input_tokens`
- provider `cache_read_tokens`
- provider `input_tokens + cache_read_tokens`
- ratio between local estimate and provider-observed context usage

This should determine whether the issue is a general estimator mismatch, a continuation-specific rendering mismatch, or a provider/Codex accounting mismatch.

### 3. Check request-shape-specific hypotheses

Evaluate, with measurements where possible:

- flattened tool-result JSON/code/log text tokenizes more densely than `chars / 4`
- per-message overhead is much larger than `4` tokens with hundreds of messages
- image blocks, if any, cost more than `IMAGE_TOKEN_ESTIMATE`
- continuation system/prompt/output reserve is not the missing overhead
- the final request includes material not represented in `content` length summaries
- Codex bridge context limit for this route is lower than the runtime's `272000`

Each hypothesis should end as: supported, rejected, or still unknown, with evidence.

### 4. Add a regression harness before fixing

Create a targeted test or diagnostic fixture that captures the failure shape without depending on prod data:

- many-message conversation
- tool-heavy outputs
- continuation rendering pipeline
- budget planner retaining too much, if that is confirmed

If the root cause turns out not to be estimator undercount, the regression should model the actual failure mechanism instead.

### 5. Recommend a fix only after evidence

Possible fixes may include, but are not limited to:

- changing estimation constants
- shape-aware token estimation for flattened tool text
- hard caps on retained messages/blocks
- request-size instrumentation
- adaptive shrink after provider rejection
- correcting the effective context window for the provider route

The implementation task should choose among these only after the investigation identifies the failing assumption.

## Acceptance criteria

- Root cause write-up separates facts from hypotheses.
- Investigation includes a reconstructed continuation request summary for the prod conversation.
- At least one targeted regression/diagnostic test exists or a precise follow-up implementation task explains why not.
- No adaptive retry or estimator policy change is implemented without evidence tying it to the observed failure.
- Follow-up fix plan is narrow and justified by the measurements.
