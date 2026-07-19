# Systems RCA for production conversation hung in `llm-requesting`

Investigate the production conversation at `/c/investigate-address-feedback-database-lock`, which remains in `llm-requesting`, and determine whether the delay is caused by Phoenix's Codex WebSocket/provider logic, a state-machine or persistence/database-lock failure, a stream-forwarding/lifecycle bug, or a genuine upstream long time to first byte.

## Investigation plan

1. Reproduce and establish a precise timeline from the affected production conversation without mutating or retrying it prematurely.
2. Query narrowly bounded VictoriaTraces data for `phoenix-ide`, identify the relevant request/turn trace IDs, and inspect full traces only for those IDs. Correlate spans with production log entries and collector warnings.
3. Follow the request end to end: UI/SSE-visible state, API handler, conversation runtime/state-machine transitions, persisted effects/messages, provider dispatch, Codex WebSocket connection and protocol events, timeout/cancellation handling, and terminal state publication.
4. Distinguish upstream latency from an internal hang using observable milestones: dispatch time, socket connect/handshake, request write, first upstream frame/byte, subsequent frames, completion/error/close, persistence commits, and SSE emission.
5. Inspect SQLite lock/busy evidence and transaction scope, including whether any database operation can block provider progress or prevent completion from becoming visible after the provider responds.
6. Audit the Codex WebSocket implementation for missing deadlines, lost wakeups, unmatched request IDs, ignored close/error frames, reconnect races, cancellation leaks, and paths that leave the runtime indefinitely in `llm-requesting`.
7. Compare the deployed behavior against local source rather than assuming parity; use production traces/logs as authoritative evidence and identify the deployed revision when possible.
8. Add focused regression coverage for the confirmed failure mode, including deterministic timeout/close/late-first-frame cases where applicable, and instrument any currently unobservable lifecycle boundary needed to make future diagnosis conclusive.
9. Implement the smallest systems-level fix that makes every dispatched request reach a structurally valid terminal outcome or a bounded, actionable failure. Avoid masking genuine upstream latency with speculative retries.
10. Validate with targeted tests plus `./dev.py check`, then document the RCA with an evidence-based timeline, root cause, contributing factors, fix, and remaining operational risks.

## Deliverables

- A conclusive classification of the incident: upstream TTFB, Codex WebSocket/provider defect, runtime/state-machine defect, SQLite contention, stream/publication defect, or a documented combination.
- Concrete production evidence supporting that conclusion.
- Code, tests, and observability improvements for any Phoenix defect found.
- If the behavior is genuinely upstream latency, a bounded timeout/status design that prevents an indistinguishable indefinite hang while preserving correct handling of slow responses.
