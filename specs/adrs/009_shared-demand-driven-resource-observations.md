# ADR-009: Native process metrics use shared demand-driven observation generations

- **Status:** Accepted
- **Date:** 2026-07-15
- **Affects:** REQ-DEPLOY-007a, REQ-PINSP-004, REQ-PINSP-008, REQ-WSUI-006, REQ-WSUI-010

## Context

Phoenix exposes native process metrics in the deployment monitor and per-handle process inspector, and the Work Scope inventory needs compact health for its live bash rows. CPU percentage requires observations separated by a minimum interval, proportional-memory reads perform operating-system work, and process attribution must validate PID start identity across that interval. Independent endpoint-owned samplers duplicate this work and can report overlapping process sets from different moments.

Sampling continuously would avoid request latency but would spend resources when nobody is viewing telemetry and would introduce a background lifecycle whose retained history and shutdown semantics exceed the needs of these point-in-time views.

## Options considered

1. **Independent request-time samplers** — each endpoint measures exactly what it needs. This is simple locally, but duplicates process discovery, CPU intervals, and proportional-memory reads across concurrent views and produces inconsistent timestamps.
2. **Permanent background monitor** — continuously sample and serve cached observations. Requests become cheap, but Phoenix consumes resources with no viewers and must own a new always-running lifecycle and history policy.
3. **Demand-driven shared observation generations** — the first request samples the authoritative union of managed process identities; concurrent and immediately following requests reuse that timestamped generation through a short bounded lease. There is no recurring task when requests stop.

## Decision

Use demand-driven shared observation generations. `ResourceMonitor` owns one bounded latest generation and coalesces concurrent requests while the sample is in flight. Every generation captures authoritative scope/handle ownership, stable native process identities, host metrics, and one deduplicated per-PID observation map. `/about`, Work Scope inventory health, and process inspectors derive their own typed projections from that map.

Bash output remains outside this generation. Output is a ring-buffer stream with cursor semantics, whereas resource metrics are point-in-time observations; combining them would force output-only consumers to pay for operating-system sampling and create parallel representations of output.

## Consequences

- **Positive:** concurrent views pay for one CPU interval and one set of proportional-memory reads; overlapping metrics share a timestamp; no viewers means no recurring sampler.
- **Positive:** Work Scope can identify hot handles without creating one sampler per row, while `/about` and inspectors retain their existing metric definitions.
- **Negative:** a consumer can receive a generation up to the short freshness lease old. Every wire projection therefore carries the generation timestamp and preserves unavailable values as null.
- **Negative:** the monitor briefly serializes callers behind the in-flight generation. A slow operating-system sample delays those callers together rather than allowing redundant samples.
- **Neutral:** ownership and observations are ephemeral in-memory aggregates. They are rebuilt from authoritative registries and native identity on the next generation and are not persisted.

## References

- `ResourceMonitor::observe`
- `ResourceObservationGeneration`
- `sample_process_observations`
- `specs/deployment-info/requirements.md`
- `specs/process-inspector/requirements.md`
- `specs/work-scope-ui/requirements.md`
