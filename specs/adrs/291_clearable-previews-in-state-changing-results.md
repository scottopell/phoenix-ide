# ADR-291: Clearable previews inside state-changing tool results

- **Status:** Accepted
- **Date:** 2026-11-27
- **Affects:** REQ-STR-002, REQ-STR-004, REQ-STR-013, REQ-PATCH-007; Allium
  `ToolResultRendering.result_clearable`, `ToolResultCleared`

## Context

Stale-result clearing removes only results whose producing tool reads
re-queryable state. `patch` is state-changing, so its results are never cleared,
yet most of each result is the applied unified diff, which `git diff` or a file
read reproduces. An offline replay over the production database (60.79B recorded
input tokens) put patch results at 4.73B carried tokens (7.8% of input). Clearing
the diff on the normal sweep schedule while keeping the receipt and hunk headers
cuts that to about 1.35–1.49B, a saving of 5.3–5.6% of input. Error results keep
their full text.

## Options considered

1. **Make `patch` clearable outright.** The largest saving, but the marker alone
   would replace the record that edits applied, which REQ-STR-002 forbids.
2. **Store the receipt and the diff as two separate fields.** Typed, but every
   consumer of the stored text (transcript reads, search, exports) would have to
   learn to reassemble it, or lose the diff.
3. **Keep the stored text whole and mark the preview's byte range with a typed
   kind.** Consumers are unchanged; only clearing interprets the span. Clearing
   validates the span (bounds, UTF-8 boundaries, success result) and keeps the
   whole result if it is invalid.

## Decision

Option 3. `ToolContent.clearable_preview` (and `ToolOutcome::Success`) carry an
optional `ClearablePreview { start, end, kind }`. `ToolOutput::with_output` and
output capping drop the span, because a span into rewritten text is meaningless.
A result with a valid preview is clearable when clearing makes it smaller;
clearing keeps the text outside the span verbatim and replaces the span with the
kind's summary (hunk headers for `UnifiedDiff`) plus the marker. The decision was
made by the reviewer and worker agents of the tool-output audit, under authority
the project owner delegated to land confirmed wins by joint agreement.

## Consequences

- **Positive:** patch diffs stop accumulating, at about 5% of input. The
  mechanism applies to any future tool that mixes a record with a preview.
- **Negative:** a byte span is checked at runtime, not by the type system.
  Invalid spans fail safe, by keeping the result. After clearing, an agent that
  needs the old diff must re-derive it with `git diff` or a file read.
- **Neutral:** rows written before this change have no span and keep their
  existing behaviour.

## References

- `docs/qa/tool-output-audit/round-2.md` (replay method and numbers)
- `ClearablePreview`, `ToolContent::valid_clearable_preview`,
  `cleared_tool_result_text`, `collect_tool_result_facts`
- specs/stale-tool-results (REQ-STR-013), specs/patch (REQ-PATCH-007)
