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

Generalization and additional compatibility machinery SHALL require demonstrated product need; an agent-authored requirement alone SHALL NOT establish that need.

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

The conversation-name transformation is owned by
`specs/conversation-identity/requirements.md` REQ-CID-006. Permanent product,
transcript, and message identities remain stable under REQ-CID-004; former
readable slugs have no retention or redirect guarantee.

**Rationale:** Forward migration supports normal upgrades without treating database damage as a supported sparse ledger. Data retention is a product decision owned by each feature. Requiring historical binaries to understand future schemas would create an unbounded cross-version protocol.

---

### REQ-COMP-003 — Database Recovery Is Offline and Feature-Scoped

THE SYSTEM SHALL NOT provide a general automatic database rollback subsystem.

WHEN an operator starts an older Phoenix binary after candidate database mutation,
THE SYSTEM SHALL require all database users to be stopped and a database backup paired with that binary to be restored before startup.

WHEN Linux deployment restores runtime artifacts without a matching database restore,
THE SYSTEM SHALL describe the result as runtime-artifact rollback and SHALL NOT claim compatibility with candidate-mutated data.

Native macOS deployment SHALL leave database recovery to the operator under REQ-LDD-030–031. Exact protected systemd recovery remains limited to REQ-PD-023–025 and REQ-PD-029. Neither path SHALL establish generic downgrade, snapshot-management or live-replacement support.

**Rationale:** Offline paired restore permits intentional downgrade without maintaining a deployment recovery framework.

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
THE SYSTEM SHALL prefer exact Kache `1.2.2` on macOS arm64 when its owned local daemon becomes ready
AND SHALL otherwise select an sccache executable that passes its version probe or no compiler cache
AND SHALL report the fallback reason and backend actually selected

WHEN Phoenix selects Kache explicitly or automatically
THE SYSTEM SHALL require a macOS arm64 host and an executable reporting exactly `kache 1.2.2`
AND SHALL generate and explicitly set both `KACHE_CONFIG` and `KACHE_HOST_CONFIG` to Phoenix-owned configuration
AND SHALL set `[cache] local_only = true` and `ignore_env = true` with explicit Phoenix-owned `local_store` and `runtime_dir`
AND SHALL NOT configure or activate remote caching, planners, or volumes through Phoenix-managed Kache
AND SHALL prevent ambient Kache remote settings and credentials from activating remote behavior
AND SHALL require local daemon readiness on the effective owned socket, binding reuse to Phoenix's socket-incarnation receipt and serving-process ownership proof
AND SHALL fail closed on an unowned or replaced socket, an unproven serving child, or an unusable local configuration
AND SHALL NOT stop or restart an existing daemon

WHEN remote Kache caching is requested
THE SYSTEM SHALL require a separate explicit remote-cache contract and qualification before enabling it
AND SHALL NOT infer remote eligibility from the local-only selection contract

WHEN an operator explicitly selects Kache
THE SYSTEM SHALL NOT silently bypass local-only policy through caller Kache configuration
AND SHALL NOT claim restored-archive source-level debug fidelity for exact `1.2.2` without release-specific native evidence

WHEN an operator explicitly selects Kache or sccache
THE SYSTEM SHALL fail actionably if that backend is unusable
AND SHALL NOT silently substitute another backend

WHEN a caller supplies `RUSTC_WRAPPER`
OR explicitly selects no compiler cache
THE SYSTEM SHALL preserve that choice
AND SHALL report it

THE SYSTEM SHALL scope automatically generated compiler-cache environment variables to direct Cargo subprocesses and explicitly identified wrappers that own Cargo builds
AND SHALL NOT propagate them into the Phoenix server or agent-executed commands

THE SYSTEM SHALL guarantee this contract only for selection, local-only configuration, daemon ownership, and local subprocess setup
AND SHALL NOT guarantee compiler-cache performance, remote-cache compatibility, cross-version cache compatibility, or restored-archive source-level debug fidelity without release-specific qualification

**Rationale:** Compiler caching is an optional development optimization. Exact version selection, a Phoenix-owned local-only boundary, and subprocess scoping prevent an accelerator from becoming an implicit correctness or remote-compatibility promise.

### REQ-COMP-012 — Withdrawn macOS compatibility qualification

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-229.

### REQ-COMP-013 — Withdrawn macOS compatibility qualification

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-229.

### REQ-COMP-014 — Exact protected systemd published-release transition

THE SYSTEM SHALL guarantee protected systemd database activation only for the complete ordered source ledger equal to the exact candidate-registry prefix through migration 65 and the fixed published migration-128 candidate identified by REQ-PD-023 in `specs/production-deployment/requirements.md`.

THE SYSTEM SHALL preserve the installed environment, service, socket, and canonical database path and require an existing predecessor. Before stopping either unit it SHALL install/fsync transaction-bound persistent service and socket start inhibits, reload systemd and verify files/effective configuration under REQ-PD-024. These inhibits SHALL survive reboot throughout disrupted pre-marker and unresolved post-marker states; tamper SHALL fail closed. Before any live candidate invocation it SHALL verify immutable evidence, exclusive offline ownership, a persistent pristine SQLite backup, and a successful exact-candidate rehearsal on an isolated backup copy under REQ-PD-024.

THE SYSTEM SHALL durably record `migration_started` immediately before the first live candidate invocation. Status SHALL remain `activating` with a marker/boolean boundary, not a separate `migration_started` state. Before that marker it MAY recover only the proven unchanged predecessor; failed/unproven pre-marker rollback SHALL retain the fence as `activation_failed_rollback_failed`. Inhibit removal SHALL occur only after authorized pre-marker unchanged-predecessor artifact restoration before restart, or immediately before exclusively verified exact-target candidate start, with verified durable removal/reload.

Explicit `recover-systemd-current` / helper `recover-current` without a marker SHALL admit only the exact active protected transaction in durable `activating` or `activation_failed_rollback_failed`, with `migration_started=false`, intact matching inhibits, validated manifest/controller/helper/rollback artifacts, and transaction-bound predecessor unit-state evidence durably captured before `activating`. Recovery SHALL stop both units, prove inactivity, resolve/probe `lsof`, and require exclusive live database integrity/foreign-key checks and the complete exact source-prefix65. Before a stopped snapshot exists, normal predecessor writes before shutdown SHALL NOT be compared to a stale preflight logical digest. Any present snapshot evidence SHALL be complete, valid, and equal to the live logical contents; partial or malformed evidence SHALL forbid restoration. Recovery SHALL restore and verify only the captured predecessor runtime/configuration, remove only its matching inhibits, restore and verify captured active/enabled state, verify predecessor identity where applicable, write durable `activation_failed_rolled_back`, and release only its exact claim. It SHALL NOT replay a database backup or enter a candidate migration/start path. Missing or ambiguous evidence SHALL retain the claim/inhibits. A crash before predecessor-state publication SHALL leave the untouched prepared claim fenced and eligible only for validated prepared abandonment, not guessed restoration.

Activation SHALL probe required `lsof` before inhibit/stop. Manual TLS SHALL validate certificate/key parsing and matching and any manual-only CA path, service readability, single CA certificate/capability and server-chain trust before disruption under REQ-PD-023–024. This is a precondition, not an authority to replace installed TLS evidence.

At or after the marker, THE SYSTEM SHALL NOT restore the backup or predecessor runtime artifacts or start the old binary, even after failure or interruption. It SHALL retain the claim and evidence on unresolved failure and permit only explicit exact-candidate forward recovery under REQ-PD-025. Marker presence, including unsafe marker evidence, SHALL select the forward path. Recovery SHALL stop both units/prove inactivity before detailed post-marker manifest, dependency, evidence or database validation, then probe `lsof`, prove exclusivity and admit only the exact retained candidate. Invalid evidence or unavailable dependencies SHALL retain the fence and attempt both-unit stop/inhibit retention, not leave a possibly old runtime running. The pristine backup SHALL remain after successful activation.

THE SYSTEM SHALL NOT infer support for any other release, baseline, target registry, prepared/local source, bare-Linux backend, downgrade, or generic rollback facility from this exception. Ordinary systemd activation through the controller enforcing REQ-PD-026 SHALL reject observed pending migrations before handoff; for an existing database it SHALL derive the complete target by executing the exact candidate's `--migrate-only` on an isolated empty DB, not source-registry parsing, and fail closed on derivation/hash/integrity/FK/ledger verification failure. A missing configured database with any installed predecessor artifact or runtime identity SHALL reject before handoff; only a true first install without predecessor evidence MAY omit the database. The helper SHALL mirror this refusal for a captured predecessor or rollback artifact. This controller-side preflight SHALL leave the protocol-v1 ordinary helper manifest unchanged; exact-target/no-pending ordinary activation remains supported. This requirement SHALL NOT establish a guarantee for historical controllers that lack that gate.

This feature-scoped backup retention does not establish a project-wide backup retention or disaster recovery service.

### REQ-COMP-015 — Withdrawn macOS compatibility qualification

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-229.

### REQ-COMP-016 — Exact protected systemd prepared transition

THE SYSTEM SHALL guarantee the protected systemd database activation defined by REQ-PD-029 only for the complete ordered source ledger equal to the exact candidate-registry prefix through migration 65 and the exact prepared target candidate `237100c79d9ce672867b284167a099fa0b7a2bcf` with embedded version `0.13.0-rc.4`, Linux target `x86_64-unknown-linux-musl`, complete registry128 source digest `a4eb047e59b21997ea068f1bd5e97c9c943f74a4c5670cdd8ff8ac10ffd5eee6`, and separately selected different controller/helper identity.

Prepared artifact provenance SHALL be a separate typed authority from the published release in REQ-COMP-014. The system SHALL require the exact closed Linux preparation receipt and payload contract in REQ-PD-029 and retain and hash-bind the receipt. Independently authenticated protected GitHub run/source/artifact association and its exact receipt-byte SHA-256 supplied through mandatory prepared-systemd option `--expected-preparation-receipt-sha256` SHALL be operator admission prerequisites and SHALL NOT be inferred from local receipt validation or hashing the local artifact directory; every other source or deployment route SHALL reject that option. The exact published RC4 tag/commit/asset/hash/registry pins and protocol-2 admission SHALL remain unchanged. An equal package version, target migration number, or semantically equal registry SHALL NOT permit source, artifact, receipt, controller or registry substitution.

The persistent inhibit, stopped exclusivity, pristine backup, isolated exact-candidate rehearsal, durable live marker, exact-target verification, pre-marker unchanged-predecessor recovery and post-marker candidate-only forward recovery guarantees in REQ-COMP-014 SHALL apply without weakening. At or after the marker, the system SHALL NOT restore the backup or predecessor runtime, start the old binary, or admit a different prepared or published candidate. Explicit forward recovery SHALL use only the exact retained candidate, receipt, registry and controller evidence and only for a same-registry prefix through migrations 65..128; invalid or unavailable evidence SHALL remain stopped, inhibited and claimed.

This exception SHALL NOT qualify another prepared artifact, candidate, architecture, backend, baseline, target, same-schema replacement, downgrade, automatic recovery or generic rollback. It SHALL NOT reinterpret the launchd prepared guarantees in REQ-COMP-012 or REQ-COMP-015 and SHALL NOT convert the published admission in REQ-COMP-014 into a generic source-kind guarantee.


### REQ-COMP-017 — Withdrawn macOS compatibility qualification

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-229.

### REQ-COMP-018 — Withdrawn macOS compatibility qualification

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-229.

### REQ-COMP-019 — Withdrawn macOS compatibility qualification

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-229.

### REQ-COMP-020 — Goals migration-lineage reconciliation

When reconciling the Goals migration suffix with the main registry, the system shall preserve main's registered migration identities and assign the Goals suffix a unique forward-only interval. Before base-schema initialization, numbered migration execution, or migration-ledger mutation, the system shall refuse databases carrying unsupported Goals-branch identities or partial, mixed, or ambiguous lineage evidence. Refusal shall leave the database unchanged; it shall not relabel ledger rows, infer missing provenance, or convert unsupported lineages. This exception does not establish a compatibility or recovery guarantee for an earlier unshipped Goals database.

The canonical registry shall preserve migrations 130 `continuation_handoff_candidates` and 131 `request_capacity_diagnostics`, followed by the unique suffix 132–140. Existing local writers shall preserve their admission, message materialization, reference, and watch-delivery behavior under that suffix. Installing this schema shall not enable peer polling, remote handlers, Goal execution, or a deployment transition.
### REQ-COMP-021 — Simple macOS forward deployment

Routine macOS deployment SHALL delegate database eligibility, migrations and reconciliation to the selected candidate. It SHALL NOT add exhaustive database scans, schema-specific admission, rehearsals, snapshot recovery, automatic predecessor startup or transaction replay. After disruption failure it SHALL retain diagnostics and ownership for manual operator recovery under REQ-LDD-030–031. No availability or downgrade guarantee follows from routine deployment.
