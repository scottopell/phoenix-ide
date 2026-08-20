# Make `commission_review` resilient to reviewer output-budget exhaustion

## Problem

`commission_review` can successfully collect and inspect a committed branch diff, spend roughly 45–48 seconds in the reviewer model, and then return no review at all when the model reaches its output-token limit. The tool reports `model_failed_no_output`, skips JSON parsing and finding extraction, and recommends retrying. Retrying repeats the same expensive failure because neither the prompt nor output strategy changes.

This is unacceptable for a capital-spend review tool: successful target/diff collection plus substantial model work must not collapse into an all-or-nothing empty result, and a retry recommendation must not send callers back through an unchanged deterministic failure mode.

## Reproduction

On branch `task-70001-expand-wide-markdown-tables`, targeting `main`, call `commission_review` on the clean committed diff.

Three consecutive attempts failed with the same shape:

- `status: failed`
- `review_status: model_failed_no_output`
- `target_collection: ok`
- `diff_collection: ok`
- `llm_review: failed`
- `json_parse: skipped`
- `finding_extraction: skipped`
- Anthropic stop reason: `max_tokens`
- `output_tokens: 4096`
- no content or tool calls returned
- elapsed time approximately 44.5s, 46.5s, and 47.9s

The third attempt explicitly narrowed the brief to the implementation files and actionable defects, but the tool still sent the full five-file branch diff, including an 86-line task document, and failed identically.

## Provider-routing diagnosis

The parent conversation was running on `gpt-5.6-sol`, but the failure text came from Anthropic. This is explained by `commission_review` selecting `ctx.llm_selector().default_service()` rather than the conversation's selected model. `ModelRegistry::pick_default_model` prefers `claude-sonnet-5`, `claude-sonnet-4-6`, and `claude-sonnet-4-5` before `gpt-5.6-sol`, so the nested capital-spend request can silently switch providers.

The tool then records usage with the synthetic model label `commission_review`, which hides the actual reviewer model/provider. Users cannot know before approval which provider will receive the diff or diagnose provider-specific failures from structured usage metadata.

## Required behavior

- Review work must produce a usable result when the reviewer exhausts its initial output budget.
- The tool must not discard generated content solely because the provider response ends with `max_tokens`.
- Structured output must be designed so the highest-value result is emitted before optional explanation. Findings/status should not depend on a long preamble fitting first.
- If one pass cannot safely review the target within budget, the tool must automatically degrade: reduce irrelevant context, partition files, request concise findings, continue generation, or use another bounded strategy.
- User-supplied focus/brief narrowing must affect the material sent for review, or the tool must expose an explicit file-scope mechanism.
- A retry recommendation must change the execution strategy; it must not recommend repeating the same request unchanged.
- Capital-spend accounting and diagnostics must distinguish provider work that yielded recoverable partial output from a true empty response.
- Approval must disclose the actual reviewer model and provider selected for the nested request.
- The result and usage metadata must record the actual reviewer model/provider, not only the synthetic `commission_review` label.
- Reviewer routing must be explicit by design: either inherit the conversation model, select a documented dedicated review model, or expose the choice to the caller. It must not silently depend on global default-model preference order.

## Acceptance criteria

- A regression test reproduces a provider response with `stop_reason=max_tokens` at the configured output limit.
- `commission_review` returns valid, trustworthy review status/findings from recoverable partial output, continuation, or automatic fallback rather than `model_failed_no_output`.
- Automatic retry/fallback is bounded and reports which strategy was used.
- Large task/spec prose cannot starve review findings for a small implementation diff.
- Repeated identical calls do not fail three times through the same unchanged path.
- Failure responses, when no recovery is possible, explain what was attempted and provide an actionable non-identical retry path.
- A conversation using `gpt-5.6-sol` cannot silently send review content to Anthropic without the approval surface and output identifying that provider/model.
