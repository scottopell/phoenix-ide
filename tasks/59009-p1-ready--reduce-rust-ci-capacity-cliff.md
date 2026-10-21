# Reduce Rust CI capacity cliff without weakening coverage

PR #902 exact-head CI demonstrates that the 5,439-test Rust suite is now close to the existing 1,500-second test-subprocess bound. Run 39520418629 attempt 2 emitted 5,096 passes, zero ordinary FAIL records, then SIGTERM after 1,500 seconds; three interrupted tests had run for only 0.3–7 seconds and 340 tests were not started. The byte-identical Rust parent passed run 39515495117, which classifies this as aggregate suite cost plus runner variance rather than a blocked federation fixture.

Evidence from the retained artifact shows broad phoenix-db stretches at roughly 1.7–2.4 seconds per test, consistent with repeated full current-schema/migration fixture setup. Diagnose and reduce aggregate wall time or introduce an existing-authority incremental qualification composition while preserving every validation contract.

Constraints:
- Do not merely increase timeout, workers, test threads, or CI machine size.
- Do not remove coverage, weaken assertions, or skip required lanes.
- Measure compile, codegen, nextest setup, and execution separately.
- Start with repeated phoenix-db current-schema initialization/migration cost and prove any fixture reuse remains isolated and correct.
- Preserve a full-suite authority boundary; parent+exact-delta evidence is acceptable only when ancestry and byte-identical unchanged surfaces are explicit.

Acceptance:
- Rust CI completes reliably inside the existing bound across repeated hosted samples, or the authorized incremental gate composes a fully qualified parent with all exact-delta lanes without omitted coverage.
- Retained artifacts still include final failure bodies and bounded diagnostic evidence.
