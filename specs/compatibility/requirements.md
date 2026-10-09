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

WHEN an older Phoenix database has no migration ledger
THE SYSTEM SHALL create the ledger and apply the opening binary's embedded migrations in version order

WHEN a database migration ledger has been written only by supported Phoenix migration and seeding tools
AND every recorded migration version is contained in the migration set embedded in the opening Phoenix binary
THE SYSTEM SHALL determine pending migrations by individual version membership rather than by a continuous prefix or highest recorded version
AND SHALL apply each unrecorded embedded migration in version order

THE SYSTEM SHALL treat a migration ledger state that was not produced by supported Phoenix migration or seeding tools as unsupported

THE SYSTEM SHALL NOT guarantee that an older Phoenix binary can open, read, or write a database after a newer binary has applied migrations that the older binary does not contain

THE SYSTEM SHALL NOT make a project-wide guarantee that migrations preserve all persisted data
AND SHALL require each migration's owning feature requirements to define whether affected data is preserved, transformed, or retired

**Rationale:** Forward migration supports normal upgrades without treating database damage as a supported sparse ledger. Data retention is a product decision owned by each feature. Requiring historical binaries to understand future schemas would create an unbounded cross-version protocol.

---

### REQ-COMP-003 — Database Recovery Is Offline and Feature-Scoped

THE SYSTEM SHALL NOT provide a general automatic database rollback subsystem

WHEN an operator rolls Phoenix back to an older binary version after candidate database mutation
THE SYSTEM SHALL require Phoenix to be stopped
AND SHALL require the database backup paired with that binary version to be restored before the binary starts

WHEN the feature-scoped paired launchd transaction fails before candidate mutation and no snapshot proof exists,
THE SYSTEM MAY resume only the verified unchanged predecessor under `specs/launchd-deployment/requirements.md` REQ-LDD-017 exclusive legacy database and captured binary/configuration proof
AND SHALL NOT describe this as a database downgrade or snapshot restoration.

WHEN an automated deployment restores a previous binary without restoring its matching previous database
THE SYSTEM SHALL describe the outcome as runtime-artifact rollback
AND SHALL NOT guarantee that the restored binary can use a database changed by the candidate

WHEN a feature requires additional automated recovery of matching runtime and database state
THE SYSTEM SHALL require that feature's normative requirements to define the recovery boundary and guarantees
AND SHALL implement only the feature-scoped recovery mechanism required by that contract

WHEN the supported macOS launchd ProductConversation upgrade is selected explicitly,
THE SYSTEM MAY provide an automated paired SQLite snapshot and rollback only within `specs/launchd-deployment/requirements.md` REQ-LDD-017 and `specs/production-deployment/requirements.md` REQ-PD-018
AND SHALL preserve the project-wide prohibition on a generic automatic database rollback subsystem
AND SHALL fail closed when exclusive ownership, snapshot integrity, or restoration proof is unavailable.

**Rationale:** Manual offline paired restore supports version rollback without a generic snapshot-management subsystem. Any additional automation would impose recovery and verification complexity and must be justified by the feature that needs it.

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

WHEN Phoenix introduces a new internal SQLite timestamp column
OR structurally changes an existing internal SQLite timestamp column
AND no more specific normative requirement identifies an external system or wire format that reads that stored value directly and requires another representation
THE SYSTEM SHALL store the timestamp in an explicitly unit-named `INTEGER` column as microseconds since the Unix epoch
AND SHALL reject non-integer values

WHEN the timestamp represents an observation made by Phoenix's current clock
THE SYSTEM SHALL reject negative values

THE SYSTEM SHALL format that integer as a human-readable date and time only at an application or presentation boundary

**Rationale:** SQLite has no native date-time storage class. New or structurally changed columns use one integer representation without forcing a project-wide migration of unchanged historical timestamp storage. The integer preserves ordering and precision without embedding a duplicate date parser or formatter contract in the schema.

---

### REQ-COMP-006 — Legacy Direct Authority Repair Is Forward-Only

WHEN a database upgrade encounters a Direct conversation whose attached `WorkScope` is classified as Restricted Explore
THE SYSTEM SHALL migrate that `WorkScope` to Direct authority
AND SHALL leave WorkScopes classified as Work unchanged
AND SHALL NOT infer a downgrade or rollback guarantee from this forward repair

**Rationale:** Direct conversations are write-authorized by contract. Repairing legacy rows restores that contract without expanding project-wide rollback guarantees.

### REQ-COMP-007 — Accepted source-locator upgrade preserves fingerprints

WHEN an accepted version-2 direct-turn payload predates source-call locators
THE SYSTEM SHALL reconstruct its original locator-absent encoding and verify its original stored fingerprint before recovery or replay.

THE SYSTEM SHALL retain the accepted payload, origin, and fingerprint without rewriting them or substituting a later invocation locator.

THE SYSTEM SHALL reject any payload whose fingerprint matches neither its current encoding nor the specifically supported historical encoding.

---

### REQ-COMP-008 — Qualified Compiler-Cache Compatibility

WHEN Phoenix automatically selects a compiler cache on macOS arm64
THE SYSTEM SHALL prefer Kache when its executable reports exactly version `1.0.0` and a Phoenix-started local daemon has a matching private identity and reports readiness on the effective configured socket
AND SHALL otherwise fall through to an sccache executable that passes its version probe or no compiler cache
AND SHALL report each fallback reason and the backend actually selected

WHEN Phoenix automatically selects a compiler cache on any other host
THE SYSTEM SHALL select an sccache executable that passes its version probe or no compiler cache
AND SHALL report the fallback reason and backend actually selected

WHEN an operator explicitly selects Kache
THE SYSTEM SHALL require a macOS arm64 host
AND SHALL require the executable to report exactly version `1.0.0`
AND SHALL require a Phoenix-started local daemon with matching private identity to report readiness on the effective configured socket

WHEN an operator explicitly selects Kache or sccache
THE SYSTEM SHALL fail actionably if that backend is unusable
AND SHALL NOT silently substitute another backend

WHEN a caller supplies `RUSTC_WRAPPER`
OR explicitly selects no compiler cache
THE SYSTEM SHALL preserve that choice
AND SHALL report it

THE SYSTEM SHALL scope automatically generated compiler-cache environment variables to direct Cargo subprocesses and explicitly identified wrappers that own Cargo builds
AND SHALL NOT propagate them into the Phoenix server or agent-executed commands

THE SYSTEM SHALL guarantee this contract only for selection and local subprocess setup
AND SHALL NOT guarantee compiler-cache performance, remote-cache compatibility, or cross-version cache compatibility

**Rationale:** Compiler caching is an optional development optimization. Exact release and host qualification, honest fallback, and subprocess scoping prevent an accelerator from becoming an implicit broad compatibility or performance promise.
