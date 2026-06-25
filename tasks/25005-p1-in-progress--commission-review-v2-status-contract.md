# Commission Review V2: partial success and trustworthy status contract

## Context

First concrete use of `commission_review` showed that the tool can produce valuable findings, but its result contract makes operational trust ambiguous. The observed output returned `status: "failed"` because an LLM request timed out, while still returning populated, useful `findings`. Callers need to know whether a review failed with no usable output or partially succeeded with actionable findings.

## YAGNI scope adjustment

Remove token/cost metadata from the `commission_review` result completely. It is confusing, currently suspect, and not useful enough to justify keeping or expanding. The capital-spend semantics are already covered by the approval gate and required executive brief. Do not add a replacement `cost` object.

Keep internal LLM usage plumbing only if Phoenix accounting requires it, but do not expose token/cost fields in the commission review JSON/display contract.

## Key insights

- `failed` must mean no actionable review output is available.
- If findings exist after a model timeout/failure, the result should be `partial`, not `failed`.
- The response should say which stage failed: target collection, diff collection, LLM review, JSON parse/repair, or finding extraction.
- Important warnings such as `model_output_repaired` should be summarized near status.
- Large finding sets need deterministic summaries: counts by severity and total findings.
- Findings should optionally include stable navigation hints such as `symbol`, in addition to fragile line numbers.

## Proposed V2 response shape

```json
{
  "status": "success | partial | failed | skipped | rejected",
  "review_status": "completed | completed_with_warnings | model_timeout_after_findings | model_timeout_no_findings | cancelled | unavailable | rejected",
  "findings_status": "complete | partial | unavailable",
  "findings_trust": "complete | partial | repaired | low",
  "target": { "...": "existing target summary" },
  "stage_status": {
    "target_collection": "ok | failed | cancelled | skipped",
    "diff_collection": "ok | failed | cancelled | skipped | truncated",
    "llm_review": "ok | partial | timeout | failed | cancelled | skipped",
    "json_parse": "ok | repaired | failed | skipped",
    "finding_extraction": "ok | partial | failed | skipped"
  },
  "finding_summary": {
    "total": 19,
    "critical": 0,
    "high": 3,
    "medium": 9,
    "low": 7
  },
  "warnings_summary": [
    "model output repaired",
    "review request timed out after partial output"
  ],
  "warnings": ["... existing typed warnings ..."],
  "findings": [
    {
      "severity": "high",
      "confidence": "high",
      "file": "path",
      "line": 155,
      "symbol": "parse_external_models",
      "title": "...",
      "rationale": "...",
      "suggested_fix": "...",
      "applicability": "current_diff | needs_verification | possibly_stale"
    }
  ],
  "retry_recommendation": "retry | do_not_retry | review_findings_first"
}
```

Exact field names can change, but the invariant should not: callers must be able to tell whether returned findings are actionable even when some review stage failed.

## Implementation plan

1. Update `specs/commission-review/requirements.md` and `design.md`:
   - Require partial success when findings exist after model/transport failure.
   - Require stage-level status reporting.
   - Require concise warning and finding summaries.
   - Require user-facing token/cost metadata to be absent from the result contract.

2. Refactor response types in `crates/phoenix-tools/src/commission_review.rs`:
   - Reserve top-level `failed` for no actionable findings/output.
   - Add typed enums for review status, stage status, findings status/trust, and retry recommendation.
   - Avoid stringly-typed operational state where invalid combinations can be structurally prevented.

3. Change chunked LLM failure handling:
   - Failure before any parsed findings or summary: `status: failed`, `findings_status: unavailable`.
   - Failure after at least one parsed finding/summary: `status: partial`, preserve findings, mark LLM stage as partial/timeout/failed.
   - Cancellation must not fabricate findings; if already-parsed findings are returned, mark the result as partial/cancelled.

4. Promote warnings into summaries:
   - Keep detailed `warnings`.
   - Add deterministic `warnings_summary` near top-level/display status.
   - Include parse-repair and timeout/failure warnings there.

5. Add deterministic finding summaries:
   - Total and severity counts after normalization/deduplication.
   - No extra LLM summarization pass.

6. Improve finding anchors:
   - Prompt/request `symbol` when available.
   - Store `symbol: Option<String>` as a navigation hint only.

7. Remove user-facing token/cost metadata:
   - Delete `input_tokens` / `output_tokens` from `ReviewSummary` serialization.
   - Do not add a `cost` object.
   - Keep internal `ToolLlmUsage` only if required by existing accounting plumbing.

8. Update display payload ordering:
   - Put status, review status, findings status, finding summary, and warnings summary before full findings.

9. Add tests:
   - Timeout before findings returns failed/unavailable.
   - Timeout after findings returns partial/available.
   - Parse repair affects trust and warnings summary.
   - Severity summary counts normalized/deduped findings.
   - User-facing token/cost fields are absent.
   - Existing skipped/rejected/dirty-worktree behavior remains intact.

## Acceptance criteria

- A model timeout after partial output no longer returns top-level `status: "failed"` with populated findings.
- Callers can distinguish no-output failure from partial actionable review.
- Stage-level status identifies where timeout/failure happened.
- Important warnings are visible near status.
- Finding severity counts are available without scanning all findings.
- Token/cost metadata is removed from the commission review result contract.
