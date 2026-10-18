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

WHEN Phoenix automatically selects a compiler cache
THE SYSTEM SHALL select an sccache executable that passes its version probe or no compiler cache
AND SHALL report the fallback reason and backend actually selected
AND SHALL NOT automatically select Kache v0.26.0

WHEN an operator explicitly selects Kache
THE SYSTEM SHALL require a macOS arm64 host
AND SHALL require the executable to report exactly version `0.26.0`
AND SHALL require its local daemon to report readiness on the configured socket
AND SHALL report that debug-symbol fidelity remains unqualified

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
AND SHALL NOT guarantee compiler-cache performance, remote-cache compatibility, cross-version cache compatibility, or restored-archive source-level debug fidelity

**Rationale:** Compiler caching is an optional development optimization. Exact qualification, explicit opt-in for unqualified fidelity, and subprocess scoping prevent an accelerator from becoming an implicit correctness or compatibility promise.

### REQ-COMP-012: Qualified irreversible current-schema transition

The system SHALL treat the migration-123 production baseline to the exact candidate migration-128 registry as an explicitly qualified prepared-artifact transition, not as generic forward compatibility. The candidate registry SHALL identify migration 126 as `retire_wake_feature_preserving_history`, migration 127 as `coordinator_owned_work_scope_home`, and migration 128 as `product_creation_provisioning_stage`; any different, missing, or newer registry SHALL fail closed. The system SHALL bind the complete candidate registry and its digest as immutable transaction evidence and SHALL re-verify that evidence before migration, activation, or forward recovery.

Before offline candidate migration is invoked, rollback MAY restore only the transaction-bound predecessor snapshot/runtime pair after proving unchanged source ledger and logical contents under exclusive offline ownership. At or after invocation, the system SHALL prohibit snapshot replay or predecessor startup, including after failed or interrupted migration, partial commits, or ambiguous reconciliation writes. Any committed migration or reconciliation write makes the predecessor database/runtime pair incompatible for automatic activation. This guarantee does not authorize downgrade, later-writer restore, mixed-version live operation, or compatibility for any other schema interval.

WHEN a more-specific prepared-deployment requirement admits a source ledger already exactly equal to that migration-128 candidate registry,
THE SYSTEM SHALL retain the same offline snapshot, mandatory candidate invocation, exact target verification, and post-invocation forward-only boundary
AND SHALL treat that admission as one bounded prepared replacement, not as another schema interval or a generic cross-binary compatibility guarantee.

WHEN an explicitly selected protected published-release current-schema transition uses a source ledger exactly equal to the complete candidate-registry prefix through migration 114,
THE SYSTEM SHALL admit only the exact migration-128 target registry defined above and an exact immutable release identity under `specs/launchd-deployment/requirements.md` REQ-LDD-019
AND SHALL bind the activation helper to a separately explicit full controller commit, not the candidate release commit, retaining its bytes and SHA-256 while obtaining the candidate binary and complete registry from the release commit
AND SHALL retain the same capacity, exclusive offline snapshot, durable `migration_started`, mandatory candidate invocation, exact target verification, and candidate-only post-invocation recovery boundaries.

This admission SHALL NOT authorize another source baseline, a source range, a future registry, generic release compatibility, or automatic post-invocation database rollback. Prepared exact-123 and exact-target admission remain distinct from this exact-114 published-release transition.

### REQ-COMP-013: Ordinary forward activation is not downgrade-safe after candidate start

WHEN an ordinary launchd deployment has pending migrations outside an explicitly qualified ordinary interval,
THE SYSTEM SHALL reject it before runtime disruption
AND SHALL NOT infer qualification from the candidate's highest migration version, general forward-migration support, or protected current-schema admission.

The exact-114 published-release transition SHALL require the protected current-schema route; it SHALL NOT extend the ordinary interval.

For the qualified ordinary in-place transition from the migration-123 baseline to the exact candidate migration-128 registry, the system SHALL treat the first candidate startup attempt as the conservative predecessor-compatibility boundary. Before that boundary, runtime artifact rollback MAY restart the unchanged predecessor database. At or after that boundary, failure or uncertainty SHALL retain the exact candidate and fail stopped/forward-only; it SHALL NOT infer predecessor compatibility from failed health, unchanged highest ledger, or absent production row evidence.

This policy SHALL NOT require a standalone backup for ordinary forward deployment and SHALL NOT claim automatic database rollback. Explicit forward recovery SHALL use only the exact retained candidate and SHALL re-verify the immutable complete registry and digest qualified by REQ-COMP-012; any different, missing, or newer registry SHALL fail closed. The wake-retirement metadata and coordinator-home environment kind remain unsupported by the predecessor unless a separate compatibility qualification proves otherwise.

### REQ-COMP-014 — Exact protected systemd published-release transition

THE SYSTEM SHALL guarantee protected systemd database activation only for the complete ordered source ledger equal to the exact candidate-registry prefix through migration 65 and the fixed published migration-128 candidate identified by REQ-PD-023 in `specs/production-deployment/requirements.md`.

THE SYSTEM SHALL preserve the installed environment, service, socket, and canonical database path and require an existing predecessor. Before stopping either unit it SHALL install/fsync transaction-bound persistent service and socket start inhibits, reload systemd and verify files/effective configuration under REQ-PD-024. These inhibits SHALL survive reboot throughout disrupted pre-marker and unresolved post-marker states; tamper SHALL fail closed. Before any live candidate invocation it SHALL verify immutable evidence, exclusive offline ownership, a persistent pristine SQLite backup, and a successful exact-candidate rehearsal on an isolated backup copy under REQ-PD-024.

THE SYSTEM SHALL durably record `migration_started` immediately before the first live candidate invocation. Status SHALL remain `activating` with a marker/boolean boundary, not a separate `migration_started` state. Before that marker it MAY recover only the proven unchanged predecessor; failed/unproven pre-marker rollback SHALL retain the fence as `activation_failed_rollback_failed`. Inhibit removal SHALL occur only after authorized pre-marker unchanged-predecessor artifact restoration before restart, or immediately before exclusively verified exact-target candidate start, with verified durable removal/reload.

Explicit `recover-systemd-current` / helper `recover-current` without a marker SHALL admit only the exact active protected transaction in durable `activating` or `activation_failed_rollback_failed`, with `migration_started=false`, intact matching inhibits, validated manifest/controller/helper/rollback artifacts, and transaction-bound predecessor unit-state evidence durably captured before `activating`. Recovery SHALL stop both units, prove inactivity, resolve/probe `lsof`, and require exclusive live database integrity/foreign-key checks and the complete exact source-prefix65. Before a stopped snapshot exists, normal predecessor writes before shutdown SHALL NOT be compared to a stale preflight logical digest. Any present snapshot evidence SHALL be complete, valid, and equal to the live logical contents; partial or malformed evidence SHALL forbid restoration. Recovery SHALL restore and verify only the captured predecessor runtime/configuration, remove only its matching inhibits, restore and verify captured active/enabled state, verify predecessor identity where applicable, write durable `activation_failed_rolled_back`, and release only its exact claim. It SHALL NOT replay a database backup or enter a candidate migration/start path. Missing or ambiguous evidence SHALL retain the claim/inhibits. A crash before predecessor-state publication SHALL leave the untouched prepared claim fenced and eligible only for validated prepared abandonment, not guessed restoration.

Activation SHALL probe required `lsof` before inhibit/stop. Manual TLS SHALL validate certificate/key parsing and matching and any manual-only CA path, service readability, single CA certificate/capability and server-chain trust before disruption under REQ-PD-023–024. This is a precondition, not an authority to replace installed TLS evidence.

At or after the marker, THE SYSTEM SHALL NOT restore the backup or predecessor runtime artifacts or start the old binary, even after failure or interruption. It SHALL retain the claim and evidence on unresolved failure and permit only explicit exact-candidate forward recovery under REQ-PD-025. Marker presence, including unsafe marker evidence, SHALL select the forward path. Recovery SHALL stop both units/prove inactivity before detailed post-marker manifest, dependency, evidence or database validation, then probe `lsof`, prove exclusivity and admit only the exact retained candidate. Invalid evidence or unavailable dependencies SHALL retain the fence and attempt both-unit stop/inhibit retention, not leave a possibly old runtime running. The pristine backup SHALL remain after successful activation.

THE SYSTEM SHALL NOT infer support for any other release, baseline, target registry, prepared/local source, bare-Linux backend, downgrade, or generic rollback facility from this exception. Ordinary systemd activation through the controller enforcing REQ-PD-026 SHALL reject observed pending migrations before handoff; for an existing database it SHALL derive the complete target by executing the exact candidate's `--migrate-only` on an isolated empty DB, not source-registry parsing, and fail closed on derivation/hash/integrity/FK/ledger verification failure. A missing configured database with any installed predecessor artifact or runtime identity SHALL reject before handoff; only a true first install without predecessor evidence MAY omit the database. The helper SHALL mirror this refusal for a captured predecessor or rollback artifact. This controller-side preflight SHALL leave the protocol-v1 ordinary helper manifest unchanged; exact-target/no-pending ordinary activation remains supported. This requirement SHALL NOT establish a guarantee for historical controllers that lack that gate.

This feature-scoped backup retention does not establish a project-wide backup retention or disaster recovery service.


### REQ-COMP-015 — Public Literal Chat Fails Closed Across Capability Loss

The public literal-chat contract is identified only by boolean `chat_literal_text_v1: true` on `GET /api/version` and the dedicated `POST /api/conversations/{id}/chat/literal` route defined by `specs/api/requirements.md` REQ-API-030. A release number, local source, or an unknown request field accepted by a server SHALL NOT establish support.

WHEN capability is missing, false, malformed, redirected, or otherwise uncertain
THE public literal-chat recipe SHALL stop before sending a chat POST
AND SHALL NOT fall back to `/chat` or an expanding policy.

WHEN a receiver without the dedicated literal route handles a request after capability discovery
THE client SHALL treat route refusal as failure, not permission to use `/chat`
AND SHALL NOT claim that a request field on `/chat` prevents a historical receiver from ignoring it.

WHEN replay evidence consists only of matching historical transcript content without policy-bearing acceptance evidence
THE SYSTEM SHALL retain default `expand_references` replay semantics
AND SHALL reject nondefault-policy replay as an idempotency conflict.

This guarantee is bounded to fail-closed client guidance, the dedicated public endpoint and policy-proven replay. It SHALL NOT establish database downgrade, historical binary support, mixed-version database ownership, runtime replacement, deployment, or automatic recovery guarantees. Capability discovery SHALL NOT authorize an operation or replace destination authentication, TLS verification, active WorkScope or lifecycle admission.
