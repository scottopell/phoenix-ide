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

## Required behavior

- Review work must produce a usable result when the reviewer exhausts its initial output budget.
- The tool must not discard generated content solely because the provider response ends with `max_tokens`.
- Structured output must be designed so the highest-value result is emitted before optional explanation. Findings/status should not depend on a long preamble fitting first.
- If one pass cannot safely review the target within budget, the tool must automatically degrade: reduce irrelevant context, partition files, request concise findings, continue generation, or use another bounded strategy.
- User-supplied focus/brief narrowing must affect the material sent for review, or the tool must expose an explicit file-scope mechanism.
- A retry recommendation must change the execution strategy; it must not recommend repeating the same request unchanged.
- Capital-spend accounting and diagnostics must distinguish provider work that yielded recoverable partial output from a true empty response.

## Acceptance criteria

- A regression test reproduces a provider response with `stop_reason=max_tokens` at the configured output limit.
- `commission_review` returns valid, trustworthy review status/findings from recoverable partial output, continuation, or automatic fallback rather than `model_failed_no_output`.
- Automatic retry/fallback is bounded and reports which strategy was used.
- Large task/spec prose cannot starve review findings for a small implementation diff.
- Repeated identical calls do not fail three times through the same unchanged path.
- Failure responses, when no recovery is possible, explain what was attempted and provide an actionable non-identical retry path.
