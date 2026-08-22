# SQLite Workload Attribution

## User Story

As a Phoenix operator diagnosing local SQLite saturation, I need a bounded in-process workload attribution model that preserves existing sparse incident telemetry while accumulating privacy-safe aggregate counters in memory so that I can later inspect recent writer occupancy, read load, and contention without depending on exported traces.

## Requirements

### REQ-SWA-001: Fixed in-memory minute ring

WHEN Phoenix initializes its SQLite workload attribution collector
THE SYSTEM SHALL allocate a fixed-size in-memory ring of 1,441 one-minute buckets
AND SHALL treat the buckets as the authoritative source for future local workload snapshots
AND SHALL keep the ring process-local and non-persistent
AND SHALL avoid dynamic labels, stored raw events, background compaction, and SQLite writes for the collector itself

---

### REQ-SWA-002: Closed attribution dimensions

WHEN the collector accepts workload observations
THE SYSTEM SHALL classify them using closed semantic category, access kind, and outcome vocabularies
AND the semantic categories SHALL be `message_persistence`, `durable_workflows`, `fts`, `runtime_state`, `pr_project_data`, `maintenance`, and `other`
AND the access kinds SHALL be `read` and `write`
AND the outcome vocabulary SHALL distinguish success, SQLite `BUSY`, SQLite `LOCKED`, pool timeout, other timeout, other failure, and abandoned observation
AND the collector MAY retain primary and extended SQLite result codes internally without exposing unbounded labels

---

### REQ-SWA-003: Fixed latency bins

WHEN the collector records operation latency distributions
THE SYSTEM SHALL use fixed latency bins with bounded cardinality
AND SHALL not emit or persist raw per-operation durations
AND SHALL record the same bounded histogram shape for every bucket rather than constructing labels from measured values

---

### REQ-SWA-004: Boundary-safe duration attribution

WHEN the collector records writer-held or read-connection durations
THE SYSTEM SHALL split each duration across every one-minute bucket it overlaps
AND SHALL attribute only the overlapped fraction to each bucket
AND SHALL assign counts and latency samples to the completion bucket
AND SHALL prevent minute-boundary overlap from inflating selectable-window occupancy

---

### REQ-SWA-005: Separate writer and read duration semantics

WHEN the collector records successful SQLite operation envelopes
THE SYSTEM SHALL track writer-held duration separately from read-connection duration
AND SHALL not relabel read duration as writer occupancy
AND SHALL preserve the invariant that writer occupancy represents only admitted write ownership time while read duration represents connection-seconds that may exceed wall time

---

### REQ-SWA-006: Fixed snapshot windows

WHEN a consumer snapshots the collector
THE SYSTEM SHALL support fixed 1 hour, 6 hour, and 24 hour windows
AND SHALL compute each snapshot from the same shared ring without mutating prior buckets
AND SHALL report the actual covered duration relative to process uptime rather than fabricating full-window coverage after restart

---

### REQ-SWA-007: Shared Database ownership

WHEN a `phoenix_db::Database` instance is created or cloned
THE SYSTEM SHALL own one shared SQLite workload attribution collector alongside the pool
AND every clone of `Database` SHALL share the same collector instance rather than copying bucket state

---

### REQ-SWA-008: Existing sparse SQLite telemetry remains available

WHEN this first SQLite workload attribution slice is added
THE SYSTEM SHALL preserve existing `sqlite_telemetry` slow-operation and failure behavior
AND SHALL not require query call-site instrumentation, API surfaces, or UI surfaces to exist before the collector module itself ships
AND SHALL allow the collector to remain present but idle until later slices connect production observations

---

### REQ-SWA-009: Unit-verifiable collector behavior

WHEN the collector is implemented
THE SYSTEM SHALL include unit tests covering bucket sizing, closed vocabularies, fixed latency bins, minute-boundary splitting for writer and read durations, fixed 1h/6h/24h snapshots, and shared-collector cloning through `Database`
