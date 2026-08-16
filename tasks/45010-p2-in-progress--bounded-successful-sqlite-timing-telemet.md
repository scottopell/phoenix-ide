# Add bounded successful SQLite timing telemetry

Measure successful-but-slow SQLite pool acquisition and transaction duration without widening failure telemetry or changing database behavior.

## Scope

- Add bounded or sampled successful-path measurements for pool acquisition and transaction duration.
- Define explicit sampling, event-volume, and attribute-cardinality limits before implementation.
- Use a closed operation vocabulary; do not attach conversation IDs, message IDs, SQL text, payloads, paths, or other high-cardinality values.
- Exclude retry backoff and time outside the transaction boundary from transaction duration.
- Keep failure-only `db.failure` telemetry authoritative for failed operations.
- Change no retry policy, busy timeout, lease, transaction semantics, or lock behavior.
- Review PR #625 only as prior art; do not import its code or broader workflow changes.

## Verification

- Deterministic tests prove acquisition and transaction clocks start and stop at the intended boundaries.
- Tests prove retry backoff is excluded.
- Tests prove the sampling/volume bound and attribute vocabulary.
- Benchmark or load evidence establishes acceptable overhead when telemetry is enabled.

## Evidence-gated plan

- Start with `direct_turn.terminal_settlement`, the operation implicated by the production incident. Do not instrument every SQLite caller.
- Measure pool acquisition from immediately before `begin_tx` until it succeeds. Measure transaction-held time from that success until a successful commit or explicit rollback completes.
- Emit at most one `db.slow_operation` span for a successful settlement when acquisition is at least 100 ms or transaction-held time is at least 250 ms. Successful fast settlements emit nothing.
- Reuse the closed `SqliteOperation` vocabulary. Export only system, operation, outcome, acquisition wait, and transaction duration; no identifiers, SQL, paths, or payloads.
- Failed acquisition, statements, rollback, or commit remain represented only by `db.failure`. Retry sleeps occur outside each observed transaction attempt and are not included.
- Verify the timing decision with deterministic injected durations, then verify the real settlement wrappers, focused tests, full checks, and a narrow local VictoriaTraces query.

## Complexity gate

This batch adds no generic SQL instrumentation, per-statement timing, retry accounting, database framework, or durable telemetry policy. The fast path pays for clock reads and small stack state only; it creates no tracing span or event. Expand the operation set only when production traces identify another semantic owner.

## Overhead evidence

A release-mode five-million-iteration measurement of the complete fast telemetry path (timer construction, five monotonic clock reads, threshold classification, and no emitted signal) measured 124 ns per operation on the development host. The benchmark-only test was removed after measurement.

## Local trace evidence

A transport smoke test exported `db.slow_operation` under `conversation.turn` with service `phoenix-ide` to local VictoriaTraces. A 10-minute query limited to 10 results returned trace `ce8baaaceb8e65db97cfa8e853a2d30a`; a numeric `span.db.pool_acquisition_ms = 9876` query matched it, and the exact trace contained only the bounded database fields. The collector logged no warning for the accepted insert. The smoke-only test was removed after verification.
