# SQLite Workload Attribution - Executive Summary

## Requirements Summary

SQLite workload attribution defines a bounded, privacy-safe, in-memory diagnostics surface for recent SQLite behavior. The contract centers on a fixed process-local 1,441-minute ring, closed category/access/outcome vocabularies, fixed bounded histograms, half-open minute-window attribution, distinct pool/admission/writer/read timing semantics, bounded percentile upper bounds, sparse incident telemetry preserved alongside aggregate reporting, and read-only 1h/6h/24h diagnostics through the deployment endpoint and About Deployment UI.

## Technical Summary

The collector lives in `phoenix-db` and is shared by every clone of `phoenix_db::Database` through an `Arc`-backed in-memory collector. Native connection hooks record all successfully profiled statements, semantic categories derived from bounded schema-object authorization, fixed histograms, writer-held time for observable transactions, profiled read execution time, and bounded read concurrency. Typed telemetry adds pool/admission failures and abandonment outcomes. Retry fields are nullable at the report boundary because broad caller-controlled retry/backoff attribution is not yet available. Counts and histograms land in the completion bucket, while writer-held and read-connection durations are split across overlapped minute buckets using half-open semantics. `phoenix-ide` samples aggregate reports from the shared collector for fixed 1h, 6h, and 24h windows, derives bounded percentile upper bounds from the histograms, and serves the result through `/api/deployment/sqlite-workload`, which the About Deployment page renders as an in-memory diagnostic report. Reports include coverage metadata, restart truncation, and process-local confidence signals instead of fabricating pre-restart history.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-SWA-001:** Fixed process-local minute ring | ✅ Complete | `phoenix-db` uses a fixed 1,441-bucket in-memory ring with slot overwrite and no collector persistence. |
| **REQ-SWA-002:** Closed attribution vocabulary | ✅ Complete | Category, access-kind, and outcome enums are closed and shared across collector and report code. |
| **REQ-SWA-003:** Fixed bounded histograms | ✅ Complete | Latency, pool-wait, and write-admission histograms use one fixed bounded bin set. |
| **REQ-SWA-004:** Completion-bucket counts with overlap-safe duration splitting | ◐ Partial | Completion counts stay in the completion minute; writer-held and native profiled-read execution durations split by overlapped fraction with half-open boundary behavior. The broader connection-held read envelope is not yet measured. |
| **REQ-SWA-005:** Separate pool, admission, writer, and read semantics | ◐ Partial | Writer/read timings and typed pool/admission failures are separate. Native PROFILE does not report failed BUSY statements, and caller-controlled retry/backoff remains unavailable rather than fabricated as zero. |
| **REQ-SWA-006:** Aggregate concurrency semantics | ◐ Partial | Native statement hooks record peak concurrency across profiled read execution intervals. SQLite's single-writer invariant bounds writer concurrency, while the broader connection-held read envelope and failed attempts not surfaced by PROFILE remain outside native concurrency samples. |
| **REQ-SWA-007:** Fixed diagnostic windows from a shared collector | ✅ Complete | The deployment API and About Deployment UI expose read-only in-memory 1h/6h/24h reports from the shared collector. |
| **REQ-SWA-008:** Coverage, restart truncation, and minimum confidence | ✅ Complete | Reports include `restart_truncated`, process start, uptime, covered uptime, and covered bucket counts. |
| **REQ-SWA-009:** Bounded percentile upper bounds and unavailable semantics | ✅ Complete | Percentiles are derived from histograms, returned as upper bounds, and become unavailable for empty histograms or the open-ended top bin. |
| **REQ-SWA-010:** Shared collector ownership across Database clones | ✅ Complete | `Database::clone` shares one collector instance rather than copying history. |
| **REQ-SWA-011:** Sparse incident telemetry remains available | ✅ Complete | Existing `sqlite_telemetry` slow-operation and failure logs/spans remain active beside aggregate reporting. |
| **REQ-SWA-012:** Privacy-safe diagnostics surface | ✅ Complete | The endpoint and UI expose aggregate counters and bounded summaries only; no SQL text or per-operation payloads are reported. |
| **REQ-SWA-013:** Verification covers collector contract and surfaces | ✅ Complete | Tests cover ring/window math, privacy and boundedness, native read/write classification, cached/colliding statements, transaction occupancy, holder/victim contention, FTS precedence, abandoned typed operations, API codegen, and UI window/stale states. |

## Coverage Notes

The collector/report contract is implemented end to end. Every connection created by `Database` installs native hooks, so raw pool clones and repository-owned pool handles retain baseline coverage. The authorizer maps known schema objects into closed categories without retaining SQL or bindings; bounded cache collisions and unobservable/autocommit writer intervals increment explicit confidence gaps. SQLite's PROFILE callback does not fire for failed BUSY statements, so broad BUSY/retry/backoff attribution cannot be inferred from the native hook. Existing typed FTS and selected transaction telemetry still records those failures; unsupported retry summaries are returned as unavailable rather than zero.
