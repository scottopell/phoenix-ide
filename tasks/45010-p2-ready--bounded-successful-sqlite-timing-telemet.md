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
