# Triage production traces for user-blocking operations

Analyze Phoenix production traces to identify slow operations that prevent users from progressing through critical journeys, distinguish endpoint latency from downstream/runtime latency, and produce an actionable migration queue for the durable-workflow stack.

## Goals

- Query the local VictoriaTraces instance using narrow time windows and bounded TraceQL results, fetching full traces only after identifying relevant trace IDs.
- Map frontend/API critical journeys to the endpoints and synchronous operations they invoke.
- Identify and rank user-blocking spans by frequency, latency, critical-path contribution, and user impact.
- Separate findings into:
  1. endpoints that should move to inbox/outbox or durable asynchronous execution;
  2. synchronous work that should remain synchronous but needs performance improvement;
  3. missing or inadequate tracing that prevents a confident diagnosis.
- Treat PR 485's durable-workflow direction as the target architecture while keeping handoff recommendations concrete and independently actionable.

## Investigation

1. Verify production trace availability and collector health at the configured local VictoriaTraces endpoint; inspect collector/application warnings rather than assuming deployed behavior matches this worktree.
2. Inventory frontend-used endpoints and the critical journeys they support, then correlate them with production service traces for `phoenix-ide`.
3. Establish representative latency distributions and inspect slow exemplars, including parent/child timing, waits, database work, Git/filesystem/process calls, provider calls, locks, polling, serialization, and other blocking dependencies.
4. Identify operations that return only after background-capable work completes, and describe the user-visible progress contract an inbox/outbox conversion would require.
5. Audit trace coverage encountered during the investigation. Improve instrumentation where low-risk and useful, including operation naming, endpoint/journey attributes, outcome/error metadata, queue/wait boundaries, causal links, and missing child spans—without introducing sensitive or high-cardinality data.
6. Validate material code or instrumentation changes with focused tests and `./dev.py check`; when practical, verify new trace data against a running environment.

## Deliverables

- A ranked evidence table with journey, endpoint/operation, sample count, observed latency, blocking reason, representative trace IDs/time range, confidence, and recommended owner/action.
- A durable-workflow handoff queue specifying candidate inbox/outbox boundaries, immediate acknowledgement behavior, durable state/progress semantics, completion/error delivery, idempotency needs, and relevant synchronous side effects.
- A performance-fix queue for slow work that should not be made asynchronous.
- A trace-coverage gap list, with high-value instrumentation improvements implemented where feasible and larger follow-ups captured explicitly.
- Reproduction/query notes sufficient to repeat the analysis while avoiding unbounded production queries or disclosure of sensitive trace contents.

## Scope controls

This is exploratory triage, not a requirement to migrate every candidate endpoint in one change. Prioritize measured production evidence and critical user progress over raw endpoint duration. Do not label an operation “blocking” merely because it is slow if the UI already acknowledges it and allows the journey to continue. Preserve correctness and existing specs; route substantial durable-workflow implementation to the durable-stack handoff rather than creating a parallel architecture.
