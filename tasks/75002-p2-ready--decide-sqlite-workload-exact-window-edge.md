# Decide SQLite workload exact-window edge policy

## Context

The process-local collector stores minute aggregates. Characterization shows that non-minute-aligned fixed windows discard the leading partial minute, include the current trailing minute without proration, and begin restart coverage at the first complete minute boundary. The timeless exact half-open edge contract therefore cannot be derived from the retained data.

## Decision needed

Choose and specify the product/storage policy for exact fixed-window edges while preserving bounded, privacy-safe diagnostics. Options may include retaining bounded edge detail, changing the aggregate representation, or revising the normative window contract through the specification/ADR process.

## Acceptance criteria

- The exact-edge behavior is decided and recorded in the appropriate normative specification and ADR.
- Storage remains bounded and privacy-safe.
- Tests cover non-minute-aligned window edges and restart-boundary coverage according to the decided policy.
