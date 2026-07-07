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

## Investigation results

### Facts confirmed

- Prod log fixed the runtime budget inputs: `context_window=272000`, `input_budget=266918`, therefore continuation fixed reserve was `5082` estimated tokens.
- A read-only reconstruction script over `/Users/scottopell/.phoenix-ide/prod.db` for conversation `365820bb-9b0d-48cc-b256-7e462b13e7c1` reproduced the logged budget behavior:
  - stored messages: `966`
  - stored content bytes: `2,060,132`
  - rendered LLM messages: `965`
  - rendered-history estimate before continuation flattening: `479,209`
  - flattened-history estimate after per-block caps: `268,459`
  - image blocks in flattened continuation history: `0`
  - budget cap result using the approximate script prompt constants: dropped `1`, kept `964`, running estimate `266,851`
  - user-first trim: trimmed `1`, final history messages `963`, final history estimate `266,711`
- The same pipeline shape is now covered in-tree by `continuation_pipeline_summary_exposes_tool_flattening_and_budget_trim`, which exercises:
  - tool-use/tool-result flattening
  - diagnostic stage summaries
  - budget trimming visibility
  - user-first trim accounting as a distinct field from budget drops

### Evidence-backed interpretation

The failure was not caused by images: the reconstructed continuation request had `0` image blocks after flattening.

The failing request was estimated extremely close to the declared window. Using the prod log's exact fixed reserve:

```text
history estimate after user-first trim: 266,711
declared fixed reserve:                 5,082
estimated request + reserved output:   271,793
context window:                       272,000
estimated headroom:                       207
```

So Phoenix did not have meaningful slack. A provider/tokenizer delta of only a few hundred tokens was enough to turn a locally "under budget" request into `context_length_exceeded`.

The large drop from `479,209` rendered-history estimate to `268,459` flattened-history estimate is explained by continuation-specific flattening and per-block caps. This transformation is intentional, but it means the continuation request is not directly comparable to prior turn `usage_data`: prior turns include real tool call/result structure, while continuation sends capped plain text without tools.

### Hypotheses evaluated

- **Flattened tool/code/log text tokenizes more densely than `chars / 4`: supported as a plausible contributor, not proven as the sole cause.** Evidence: request had only ~207 estimated tokens of slack. Any modest tokenizer/serialization mismatch can explain the provider rejection.
- **Per-message overhead much larger than `4` tokens: supported as a plausible contributor, not isolated.** Evidence: final history had `963` messages; one extra token/message would exceed the available slack several times over.
- **Image estimate too low: rejected for this case.** Evidence: reconstructed flattened continuation history had `0` images.
- **Continuation prompt/system/output reserve omitted: rejected as primary cause.** Evidence: prod log's `input_budget=266918` shows Phoenix did reserve `5082` estimated tokens for fixed prompt/system/output/safety; the issue is that the remaining slack was tiny.
- **Codex bridge effective limit lower than `272000`: still unknown.** The provider rejected the request, but the current evidence does not distinguish a lower effective limit from estimator/wire overhead mismatch.
- **`cache_read_tokens` interpretation caused this: still unknown.** Prior turns near `244k` context usage explain why continuation triggered, but continuation request tokenization was not provider-measured because it failed.

### Narrow fix recommendation

The root failing assumption is now narrower: the continuation budget planner treats an estimate within ~200 tokens of the provider cap as safe. That is not safe for a 963-message tool-heavy request.

Recommended follow-up fix: add an explicit minimum continuation headroom requirement after user-first trimming. If the retained history estimate plus fixed reserve leaves less than a conservative floor, keep dropping oldest messages until the floor is met. A starting floor of at least several thousand tokens is justified by this failure because per-message overhead alone can exceed a few hundred tokens at this message count.

This is narrower than adaptive retry and does not require assuming one specific tokenizer mismatch. Adaptive shrink-on-provider-reject may still be useful later, but it should be a separate resilience improvement rather than the primary fix for this measured failure.

### Verification performed

```bash
cargo test -p phoenix_ide continuation_pipeline_summary_exposes_tool_flattening_and_budget_trim -- --nocapture
```

Result: passed.
