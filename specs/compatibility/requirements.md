# Phoenix Compatibility Guarantees

## User Story

As a Phoenix operator, I need compatibility and recovery promises to be explicit so that upgrades, rollbacks, and persisted data handling are safe without silently committing Phoenix to unsupported cross-version or live-replacement behavior.

## Scope

This specification defines project-wide defaults for compatibility guarantees, database upgrade and rollback compatibility, replacement of a Phoenix database, and the durable representation of internal SQLite timestamps. A more specific normative specification may define an additional guarantee or an externally owned representation explicitly.

This specification does not provide backup retention, disaster recovery, point-in-time recovery, or cross-host database portability.

## Requirements

### REQ-COMP-001 — Compatibility Is Explicit

THE SYSTEM SHALL treat a compatibility, downgrade, rollback, recovery, or live-resource-replacement behavior as guaranteed only when a normative Phoenix requirement states that guarantee

WHEN no normative requirement grants such a guarantee
THE SYSTEM SHALL treat the behavior as unsupported
AND MAY refuse the operation or require offline manual recovery
AND SHALL NOT report success by assuming the unsupported behavior worked
AND SHALL NOT introduce permanent compatibility machinery solely because the behavior could be implemented defensively

**Rationale:** Accidental compatibility becomes a permanent architectural and testing obligation. Explicit requirements make its product value and full-system cost reviewable.

---

### REQ-COMP-002 — Database Compatibility Moves Forward

WHEN every migration version recorded by a Phoenix database is contained in the migration set embedded in the opening Phoenix binary
THE SYSTEM SHALL determine pending migrations by individual version membership rather than by a continuous prefix or highest recorded version
AND SHALL apply each unrecorded embedded migration in version order
AND SHALL preserve persisted data unless a normative requirement and its decision record explicitly retire that data

THE SYSTEM SHALL NOT guarantee that an older Phoenix binary can open, read, or write a database after a newer binary has applied migrations that the older binary does not contain

**Rationale:** Forward migration supports normal upgrades. Requiring historical binaries to understand future schemas would constrain every migration and create an unbounded cross-version protocol.

---

### REQ-COMP-003 — Database-Compatible Rollback Restores a Matching Pair

WHEN a deployment or recovery operation restores a previous Phoenix binary without restoring its matching previous database
THE SYSTEM SHALL describe the outcome as runtime-artifact rollback
AND SHALL NOT describe the outcome as database-compatible rollback
AND SHALL NOT guarantee that the restored binary can use a database changed by the candidate

WHEN the system reports database-compatible rollback
THE SYSTEM SHALL have restored and verified the matching previous runtime, configuration, environment, service state, and database as one recovery unit

**Rationale:** Binary rollback without data rollback can pair an older binary with a schema it does not support. Explicit outcome names prevent the existing runtime-artifact recovery path from claiming a stronger compatibility guarantee than it provides.

---

### REQ-COMP-004 — Database Replacement Is Offline

THE SYSTEM SHALL support one backend-managed Phoenix runtime version as the exclusive application owner of a production SQLite database
AND SHALL NOT support mixed-version Phoenix runtimes or independently launched Phoenix processes sharing that production database

WHEN an operator restores or replaces a Phoenix SQLite database
THE SYSTEM SHALL require the backend-managed runtime to be stopped before replacement begins
AND SHALL require the operator to ensure that no unsupported process is using the database
AND SHALL open and validate the replacement database through fresh connections after Phoenix restarts

THE SYSTEM SHALL NOT guarantee detection, fencing, or recovery when an open database file is replaced beneath a running Phoenix process

**Rationale:** Replacing an open SQLite database would require database-instance fencing across operations and connection pools. An offline replacement boundary provides deterministic ownership without that distributed protocol.

---

### REQ-COMP-005 — Internal SQLite Timestamps Use Integer Unix Microseconds

WHEN Phoenix stores a timestamp in a Phoenix-owned SQLite schema
AND no more specific normative requirement identifies an external system or wire format that reads that stored value directly and requires another representation
THE SYSTEM SHALL store the timestamp in an explicitly unit-named `INTEGER` column as microseconds since the Unix epoch
AND SHALL reject non-integer values

WHEN the timestamp represents an observation made by Phoenix's current clock
THE SYSTEM SHALL reject negative values

THE SYSTEM SHALL format that integer as a human-readable date and time only at an application or presentation boundary

**Rationale:** SQLite has no native date-time storage class. One integer representation preserves ordering and precision without embedding a duplicate date parser or formatter contract in the schema.
