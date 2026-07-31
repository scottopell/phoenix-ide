# ADR-025: Continuation compaction is an idempotent durable operation

- **Status:** Accepted
- **Date:** 2026-07-30
- **Affects:** REQ-BED-020, `ContinuationSummaryRequest`, `RecoverableContinuationFailure`

## Context

Continuation compaction crosses a provider boundary and a persistence boundary. A provider request can fail transiently, complete after a retry has begun, or be interrupted by process restart. Persisting the generated summary and exhausted state as separate effects also leaves a crash interval in which only half of the logical outcome exists.

The provider cannot guarantee exactly-once execution, while Phoenix must guarantee that one logical compaction produces at most one committed summary and one continuation handoff.

## Options considered

1. **Collapse every summary failure into context exhausted with fallback text** — simple and terminal, but loses the retry intent and mistakes failure text for a generated handoff.
2. **Retry only in runtime memory** — avoids schema changes but loses operation identity and inputs on restart, and cannot reject stale provider results.
3. **Persist an identified operation and commit its result idempotently** — permits at-least-once provider execution while making the Phoenix commit exactly once; requires typed lifecycle state and a transaction spanning summary-message and state persistence.

## Decision

Persist continuation summary generation as an explicit operation before provider dispatch. Its stable identity travels through requests and results, survives automatic and user-initiated retries, and permits stale results to be rejected.

A successful result uses a deterministic message identity derived from the operation identity and atomically commits the continuation message with the context-exhausted state. A repeated commit is a no-op. A result for any other operation is stale and cannot alter state.

Transient errors retain the same operation identity across bounded automatic retries. Exhausted or non-retryable errors enter a typed recoverable state carrying the operation inputs and failure. Explicit retry reuses the operation identity. Startup resumes an operation that was persisted as in flight; it does not automatically retry an operation whose failure is already visible to the user.

## Consequences

- **Positive:** Crashes, duplicate delivery, late results, and provider retries cannot create duplicate summaries or overwrite a newer lifecycle.
- **Positive:** Capacity and other terminal attempt failures remain actionable without fabricating a continuation summary.
- **Negative:** Continuation request and response events must carry operation identity, and persistence needs a continuation-specific atomic compare-and-commit operation.
- **Negative:** External provider requests remain at-least-once; exactly-once applies to Phoenix's logical commit and continuation handoff.
- **Neutral:** The persisted conversation state remains an indivisible polymorphic aggregate; compare-and-commit decodes and compares that aggregate rather than querying fields inside its JSON encoding. SQL lifecycle filters use a schema-constrained `state_kind` discriminator written atomically with the aggregate.

## References

- `specs/bedrock/requirements.md` REQ-BED-020
- `specs/bedrock/bedrock.allium` context continuation rules
- `Effect::ContinuationCommit`
- `Database::commit_continuation`
