# SQLite Workload Attribution

## User Story

As a Phoenix operator diagnosing local SQLite saturation, I need a bounded in-process workload attribution model that preserves sparse incident telemetry while accumulating privacy-safe aggregate counters in memory so that I can inspect recent writer occupancy, read load, contention, and retry behavior without depending on exported traces.

## Requirements

### REQ-SWA-001: Fixed process-local minute ring

WHEN Phoenix initializes SQLite workload attribution
THE SYSTEM SHALL allocate a fixed-size in-memory ring of 1,441 one-minute buckets
AND SHALL treat that ring as the authoritative source for local workload snapshots and aggregate reports
AND SHALL keep the ring process-local, non-persistent, and independent of SQLite writes by the collector itself
AND SHALL overwrite expired slots in place rather than retaining raw event history

---

### REQ-SWA-002: Closed attribution vocabulary

WHEN the collector records SQLite workload observations
THE SYSTEM SHALL classify each observation with closed vocabularies for category, access kind, and outcome
AND the category vocabulary SHALL be exactly `message_persistence`, `durable_workflows`, `fts`, `runtime_state`, `pr_project_data`, `maintenance`, and `other`
AND the access-kind vocabulary SHALL be exactly `read` and `write`
AND the outcome vocabulary SHALL be exactly `success`, `busy`, `locked`, `pool_timeout`, `other_timeout`, `other_failure`, and `abandoned`
AND the collector MAY retain SQLite primary and extended result codes only as bounded diagnostic detail outside those exposed vocabularies

---

### REQ-SWA-003: Fixed bounded histograms

WHEN the collector records latency-derived distributions
THE SYSTEM SHALL place every recorded duration into fixed bounded bins shared by every bucket, category, and access kind
AND the latency-bin vocabulary SHALL be exactly durations under 1 ms, 1-4 ms, 5-19 ms, 20-99 ms, 100-249 ms, 250-999 ms, and 1000 ms or more
AND SHALL not emit, persist, or expose raw per-operation durations as part of workload attribution

---

### REQ-SWA-004: Completion-bucket counts with overlap-safe duration splitting

WHEN the collector records an observation that completes at time `t`
THE SYSTEM SHALL record operation counts, outcome counts, and histogram samples in the one-minute bucket containing `t`
AND SHALL distribute writer-held and read-connection durations across every overlapped minute bucket using only the overlapped fraction in each bucket
AND SHALL treat bucket windows as half-open intervals so that a duration ending exactly on a minute boundary is attributed to the preceding minute rather than double-counted in the following minute
AND SHALL prevent minute-boundary overlap from inflating selectable-window occupancy or connection time

---

### REQ-SWA-005: Separate pool, admission, writer, and read semantics

WHEN the collector records a write observation
THE SYSTEM SHALL distinguish pool-acquisition wait, write-admission wait after pool acquisition, total operation latency, retry count, retry backoff, and writer-held duration as separate measures
AND SHALL use write-admission wait to represent elapsed time between pool acquisition and admitted write ownership excluding pool wait and excluding retry backoff that happened before the observed attempt
AND SHALL use writer-held duration to represent only admitted write-ownership time

WHEN the collector records a read observation
THE SYSTEM SHALL distinguish pool-acquisition wait, total operation latency, retry count, retry backoff, and read-connection duration as separate measures
AND SHALL represent read-connection duration independently from writer-held duration
AND SHALL permit summed read-connection time within a window to exceed wall-clock coverage because concurrent reads consume connection-seconds rather than writer ownership

---

### REQ-SWA-006: Aggregate concurrency semantics

WHEN the collector records workload observations
THE SYSTEM SHALL retain writer concurrency and read concurrency as bounded per-bucket peaks rather than as raw timelines
AND SHALL report writer concurrency as admitted writer ownership observed for that bucket
AND SHALL report read concurrency as simultaneous read connections observed for that bucket
AND SHALL use zero when the implementation does not observe concurrency for a recorded outcome rather than fabricating a derived value

---

### REQ-SWA-007: Fixed diagnostic windows from a shared collector

WHEN a consumer requests SQLite workload diagnostics
THE SYSTEM SHALL derive every report from the same shared minute ring using fixed windows of exactly 1 hour, 6 hours, and 24 hours
AND SHALL support both per-minute snapshots and aggregate reports over those windows without mutating stored buckets
AND SHALL expose the aggregate reports through a read-only in-memory diagnostics endpoint and UI for those same 1h, 6h, and 24h windows
AND SHALL not persist workload attribution state to disk for those diagnostics

---

### REQ-SWA-008: Coverage, restart truncation, and minimum confidence

WHEN the requested diagnostic window exceeds process uptime since the collector started
THE SYSTEM SHALL truncate coverage to the covered uptime actually available in the current process
AND SHALL mark the report as restart-truncated rather than fabricating pre-restart history
AND SHALL report process start time, process uptime, covered uptime, and covered bucket count so consumers can judge confidence
AND SHALL treat covered uptime greater than zero as the minimum confidence needed to produce a report, even when the requested window is only partially covered

---

### REQ-SWA-009: Bounded percentile upper bounds and unavailable semantics

WHEN a consumer requests percentile summaries from a workload histogram
THE SYSTEM SHALL compute percentile outputs from the fixed bounded histogram rather than from raw samples
AND SHALL report each percentile as an upper bound for the selected bin rather than as an exact duration
AND SHALL return an unavailable percentile when the histogram has no samples for that measure
AND SHALL return an unavailable percentile when the percentile falls into the open-ended 1000 ms-or-more bin because no finite upper bound exists

---

### REQ-SWA-010: Shared collector ownership across Database clones

WHEN a `phoenix_db::Database` instance is created or cloned
THE SYSTEM SHALL associate it with one SQLite workload attribution collector shared alongside the pool
AND every clone of `Database` SHALL share the same collector instance rather than copying bucket state
AND all reports produced through those clones SHALL observe the same bounded in-memory history for that process

---

### REQ-SWA-011: Sparse incident telemetry remains available

WHEN SQLite workload attribution records observations
THE SYSTEM SHALL preserve sparse `sqlite_telemetry` slow-operation and failure diagnostics as an independent incident-oriented signal
AND SHALL allow those sparse logs and spans to coexist with aggregate workload attribution without requiring raw-event export from the collector
AND SHALL keep workload attribution compatible with observations that remain uncategorized by classifying them as `other`

---

### REQ-SWA-012: Privacy-safe diagnostics surface

WHEN Phoenix exposes SQLite workload diagnostics
THE SYSTEM SHALL expose only aggregate counts, bounded histograms, bounded percentile upper bounds, fixed category labels, fixed outcome labels, and fixed-window coverage metadata
AND SHALL not expose SQL text, bound values, row contents, conversation content, file paths, primary keys, or other per-operation payload data through the workload attribution reports
AND SHALL keep any retained SQLite result-code detail bounded and diagnostic rather than user-data-bearing

---

### REQ-SWA-013: Verification covers collector contract and surfaces

WHEN SQLite workload attribution is implemented
THE SYSTEM SHALL include verification covering the fixed ring size, closed vocabularies, fixed histogram bins, half-open minute-boundary duration splitting, fixed 1h/6h/24h windows, restart truncation, percentile unavailable semantics, shared-collector cloning through `Database`, and the read-only report surface contract
AND comprehensive verification of workload category assignment, retry attribution, and concurrency attribution across every SQLite call site MAY be partial until all production observation sites are instrumented against the shared collector

---

### REQ-SWA-014: Closed logical read-family attribution contract

WHEN the collector records logical read-family attribution
THE SYSTEM SHALL classify each logical read observation with the closed read-family vocabulary `active_list`, `archived_list`, `conversation_get`, `full_history`, `latest_bounded_history`, and `recovery_range_history`
AND SHALL retain separate fixed per-minute aggregates for each read family consisting only of outcome counts, summed logical elapsed time, and logical elapsed histogram samples
AND SHALL record each logical read-family observation in the one-minute bucket containing its completion time
AND SHALL expose success, failure, and abandonment through a `#[must_use]` RAII guard returned by `start_read_family` whose success and failure methods consume the guard and whose drop path records abandonment exactly once
AND SHALL derive the guard's completion timestamp and logical elapsed duration from the shared collector clock so deterministic tests and production recording observe the same completion semantics
AND SHALL keep read-family attribution bounded by the fixed minute ring without storing per-operation history or expanding the vocabulary at runtime
