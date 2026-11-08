# ADR-243: A bounded resource recorder reuses shared observations

- **Status:** Accepted
- **Date:** 2026-11-08
- **Affects:** REQ-DEPLOY-007a–007d

## Context

Point-in-time telemetry cannot explain memory or disk pressure preceding a process
exit when no browser was watching. ADR-009 deliberately rejected a permanent monitor
because the interactive views did not require its history or lifecycle. The operator
now requests bounded private resource evidence that remains independent of SQLite
and structured-log placement, without changing startup or fatal authority behavior.
The reviewed storage leaf already bounds queue, frames and segments and rejects
unsafe filesystem entries. Native validation is resource-gated during integration.

## Options considered

1. **Keep demand-only observations** — no idle sampling cost, but no evidence between
   interactive requests or before abrupt exits.
2. **Add an independent monitor, database history or daemon** — more flexible history,
   but duplicates native process collection and introduces new storage/lifecycle
   authority and privacy risks.
3. **Make the bounded recorder a slow consumer of the existing generation** — shares
   metric attribution and freshness, with fixed private storage and modest idle cost.

## Decision

Choose option 3. Partially supersede ADR-009 only for its no-autonomous-consumer and
no-retained-history conclusions: the recorder consumes `ResourceMonitor::observe`
at a completion-spaced cadence of at least 60 seconds and shares About's typed
aggregate projection. Other clients retain demand coalescing and their own UI history.

Attempt initialization from the typed runtime data parent before DB open. Startup
prepares an absent parent using ordinary directory creation without changing existing
permissions; the storage leaf still rejects unsafe parents rather than repairing
them. Failures are nonfatal; storage I/O failure ends this process's recorder without
retry. Preserve the reviewed storage protections.
Use fixed primitives/enums and compact version-2 frames, stripping paths, names,
source identifiers, filesystem identities and raw errors. Reuse host memory refresh
for swap and deployment space sampling for fixed disk roles. The only workload count
is a nonblocking snapshot of live materialized runtime handles, not queued work.

Do not invent peak memory, host pressure or SQLite codes without supported source
observations. Keep these null, and serialize an unknown process startup timestamp
as null rather than inventing zero. Record process startup/uptime and host uptime as
limited gap evidence, not a synthetic boot identity or crash-gap reconstruction.
Compute fixed boundary/drop deltas in the writer and advance baselines only after
successful append and sync. Keep producer, appended and committed observations
separate; valid surviving frames are not proof of successful pre-crash sync.

## Consequences

- **Positive:** small path-free evidence survives ordinary process restarts without
  depending on the database or changing the authority/fatal paths.
- **Negative:** sampling consumes native collection work even with no UI clients;
  unknown intervals, final lost observations, unsupported metrics and rotation loss
  remain accepted limits. A host reporting zero swap total is ambiguous in sysinfo
  and is conservatively null, including a genuinely swap-free host.
- **Neutral:** no history API/UI, replay, database scan, shutdown sync guarantee,
  cross-version recovery promise or general monitoring framework is introduced.

## References

- [ADR-009](009_shared-demand-driven-resource-observations.md)
- [Deployment requirements](../deployment-info/requirements.md)
- `resource_flight_recorder::start`, `resource_flight_recorder::run`
- `ResourceMonitor::observe`, `deployment::resource_summary`
- `deployment_observations::recorder_space`
