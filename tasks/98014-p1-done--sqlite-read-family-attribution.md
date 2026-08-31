# Add source-defined SQLite read-family attribution

Extend the existing bounded process-local SQLite workload collector with a
privacy-safe closed read-family vocabulary so production diagnostics can
attribute logical read volume and elapsed time to source-defined operation
families without retaining SQL text, parameters, identifiers, paths, or user
content.

## Scope

- Preserve native statement/category telemetry as the total-work baseline.
- Add bounded per-minute aggregate attribution for an initial closed set:
  - active conversation list
  - archived conversation list
  - single-conversation get
  - full message history
  - bounded/latest message history
  - recovery/range message history
- Record logical attempt count, bounded elapsed-time histogram, outcome, and
  abandonment without changing query behavior.
- Expose aggregates through the existing deployment SQLite workload report.
- Keep memory and vocabulary fixed and process-local.
- Do not optimize, batch, cache, or otherwise change the underlying reads.

## Acceptance criteria

- No SQL text, parameters, conversation/message IDs, paths, or content are retained.
- Exactly one terminal observation is recorded per typed outer read attempt.
- Cancellation/drop records abandonment.
- Nested helper phases do not double-count logical attempts.
- Existing native totals remain independently visible and authoritative.
- Deterministic tests cover success, failure, abandonment, minute rollover,
  aggregation, and report serialization.
