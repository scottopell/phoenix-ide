# Fail cheap source defects before semantic CI

Gate expensive hosted qualification on an exact-source, no-compile check that reuses existing check-plan selector regressions, runs Rust formatting without compiling, and statically validates the compiled migration ledger's structural invariants. Preserve every existing semantic/build/test gate; checks that genuinely require compilation remain in their owning suites.

## Acceptance criteria

- Representative formatting, lane-selector, and migration-ledger defects fail in the early gate.
- A valid source change passes the early gate and proceeds to existing full hosted suites.
- Existing final CI aggregation and semantic coverage remain complete.
- Evidence reports measured hosted timing rather than estimated savings.
