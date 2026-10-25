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

The conversation-name transformation is owned by
`specs/conversation-identity/requirements.md` REQ-CID-006. Permanent product,
transcript, and message identities remain stable under REQ-CID-004; former
readable slugs have no retention or redirect guarantee.

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

WHEN a prepared artifact's receipt, expected full commit and embedded candidate SHA all equal `23badb8be4bc8d05a7e1c49c5367ceb14ebccd92` and its complete candidate `crates/phoenix-db/src/migrations.rs` git object has SHA-256 `a4eb047e59b21997ea068f1bd5e97c9c943f74a4c5670cdd8ff8ac10ffd5eee6`,
THE SYSTEM SHALL additionally admit the exact complete registry prefix through 114 to the exact target128 under REQ-LDD-019, only with the typed prepared-source qualification evidence defined there.
The semantic migration registry SHALL remain the RC4 registry; this is an explicit source-kind/candidate expansion, not a migration or generic prepared114 qualification. The controller SHALL be a separately recorded different clean checkout commit; candidate registry bytes SHALL come from the immutable candidate git object, never controller working-tree bytes. Missing, partial, extra or contradictory qualification fields SHALL reject before disruption. The same capacity, snapshot, exclusivity, durable `migration_started`, candidate-only post-invocation recovery and finalization rules SHALL apply. Prepared exact-123/exact-target and protected published-release114 admissions SHALL remain distinct and unchanged. No other prepared candidate, baseline, target or source kind gains source114 admission.

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

### REQ-COMP-015 — Exact current-schema candidate-only forward replacement

THE SYSTEM SHALL qualify only the following launchd forward-replacement pin set; unchanged registry semantics SHALL NOT imply general cross-binary compatibility:

| Pin | Exact value |
| --- | --- |
| Original prepared candidate | `23badb8be4bc8d05a7e1c49c5367ceb14ebccd92` |
| Repair prepared candidate | `237100c79d9ce672867b284167a099fa0b7a2bcf` |
| Repair embedded version | `0.13.0-rc.4` |
| Complete registry128 source SHA-256 | `a4eb047e59b21997ea068f1bd5e97c9c943f74a4c5670cdd8ff8ac10ffd5eee6` |

WHEN the original current-schema transaction retains its exact active claim, immutable committed status with `finalization_pending = true`, a committed exact-target128 proof, and confirmed absent service and activation helper,
THE SYSTEM MAY seal exactly one forward-replacement child under REQ-LDD-025 in `specs/launchd-deployment/requirements.md`, using only the repair prepared candidate above, the same complete target ledger, and a separate clean controller commit different from the repair candidate.

THE SYSTEM SHALL preserve the original manifest, committed/pending status, proof, backup, private candidate and private plist as audit evidence. Before sealing, the child SHALL durably copy the exact original committed/pending status to private immutable `parent-status.json` and bind its SHA-256 in the child manifest. The child SHALL NOT write the parent's global status. After full operation admission and durable child staging but before sealing, the controller SHALL atomically replace the installed original binary with a non-executable empty stopped fence and remove the published target plist. An interruption before sealing SHALL retain the original claim and stopped fence; a retry MAY accept only the exact original installed binary or that exact fence. This runtime fence SHALL make historical retained finalizers and login/reboot persistence unable to start the original candidate without modifying its private audit evidence. Unresolved work SHALL cross-check live global status against the bound snapshot and retain exact original ownership; terminal historical audit SHALL use the snapshot after another transaction replaces global status. Lightweight sealed-child classification SHALL validate stored parent manifest/status/proof hashes and exact shapes/paths without scanning the live database or backup; operation admission SHALL perform full parent backup/proof validation and offline database integrity/foreign-key/full-ledger checks before runtime mutation. Sealing the deterministic child SHALL prohibit original-candidate finalization even when child evidence is malformed; only the same sealed child MAY resume. Child invocation SHALL never replay any snapshot or start the original candidate or predecessor, even before repair reconciliation, on failure, or after ambiguous writes. Repair reconciliation SHALL use the exact installed repair binary after a durable `reconciliation_started` checkpoint; target verification and runtime identity SHALL precede successful publication and original-claim release. Terminal child status SHALL remain unresolved while the exact parent claim exists, so another operation cannot reclaim ownership between terminal-status durability and claim release. This exception SHALL NOT authorize other candidates, schema intervals, generic rollback, another backend, or automatic recovery without exact retained evidence.


### REQ-COMP-016 — Exact protected systemd prepared transition

THE SYSTEM SHALL guarantee the protected systemd database activation defined by REQ-PD-029 only for the complete ordered source ledger equal to the exact candidate-registry prefix through migration 65 and the exact prepared target candidate `237100c79d9ce672867b284167a099fa0b7a2bcf` with embedded version `0.13.0-rc.4`, Linux target `x86_64-unknown-linux-musl`, complete registry128 source digest `a4eb047e59b21997ea068f1bd5e97c9c943f74a4c5670cdd8ff8ac10ffd5eee6`, and separately selected different controller/helper identity.

Prepared artifact provenance SHALL be a separate typed authority from the published release in REQ-COMP-014. The system SHALL require the exact closed Linux preparation receipt and payload contract in REQ-PD-029 and retain and hash-bind the receipt. Independently authenticated protected GitHub run/source/artifact association and its exact receipt-byte SHA-256 supplied through mandatory prepared-systemd option `--expected-preparation-receipt-sha256` SHALL be operator admission prerequisites and SHALL NOT be inferred from local receipt validation or hashing the local artifact directory; every other source or deployment route SHALL reject that option. The exact published RC4 tag/commit/asset/hash/registry pins and protocol-2 admission SHALL remain unchanged. An equal package version, target migration number, or semantically equal registry SHALL NOT permit source, artifact, receipt, controller or registry substitution.

The persistent inhibit, stopped exclusivity, pristine backup, isolated exact-candidate rehearsal, durable live marker, exact-target verification, pre-marker unchanged-predecessor recovery and post-marker candidate-only forward recovery guarantees in REQ-COMP-014 SHALL apply without weakening. At or after the marker, the system SHALL NOT restore the backup or predecessor runtime, start the old binary, or admit a different prepared or published candidate. Explicit forward recovery SHALL use only the exact retained candidate, receipt, registry and controller evidence and only for a same-registry prefix through migrations 65..128; invalid or unavailable evidence SHALL remain stopped, inhibited and claimed.

This exception SHALL NOT qualify another prepared artifact, candidate, architecture, backend, baseline, target, same-schema replacement, downgrade, automatic recovery or generic rollback. It SHALL NOT reinterpret the launchd prepared guarantees in REQ-COMP-012 or REQ-COMP-015 and SHALL NOT convert the published admission in REQ-COMP-014 into a generic source-kind guarantee.


### REQ-COMP-017 — Exact protected launchd schema-128-to-129 transition

THE SYSTEM SHALL guarantee native macOS launchd database activation for exactly one protected transition: a complete ordered source ledger equal to the qualified migration-128 registry and a complete ordered target ledger equal to the exact route-pinned migration-129 registry. The route SHALL bind the complete target registry and digest, the candidate's exact full embedded identity, the exact package version, the retained helper identity and hash, the canonical database path, and the transaction identity as immutable evidence. A version number, maximum ledger version, unchanged migration prefix, or semantically similar registry SHALL NOT establish this route's identity. Existing migration-128 route pins and their source-kind distinctions SHALL remain unchanged.

Before any live candidate database write, the route SHALL reserve capacity for both the retained backup and one restore-capacity slot, durably quarantine the canonical auto-loaded LaunchAgent plist, quiesce the predecessor, prove exclusive ownership of the database, WAL, and SHM, and create a native SQLite backup-API snapshot that is integrity-checked, foreign-key-checked, fsynced, and bound to the exact source ledger, logical contents, route registry, candidate, helper, predecessor, and transaction. The exact candidate SHALL be rehearsed sequentially against a copy in that restore-capacity slot: the rehearsal SHALL complete and be verified before any live candidate invocation, SHALL not run concurrently with backup creation or another copy, and SHALL not consume unreserved capacity. Rehearsal failure, capacity failure, malformed proof, or loss of exclusivity SHALL remain pre-marker failures.

The route SHALL prove the predecessor service, activation helper, and candidate process are absent after quarantine and retain the quarantine and ownership evidence in the private transaction. Candidate startup SHALL use only the exact retained private plist and exact retained candidate; persistent publication SHALL occur only after exact runtime identity and health verification. No startup path may proceed while another Phoenix process owns the database or while target-path quarantine/exclusivity is unresolved.

The route SHALL durably record `migration_started` immediately before the first live candidate migration or reconciliation invocation. Before that marker, recovery MAY restore the exact captured predecessor runtime/configuration and restart only that predecessor after re-proving exclusive ownership, exact source ledger, and unchanged logical contents; it SHALL not manufacture that proof by replaying the backup. At or after the marker, including interruption, partial commit, failed invocation, or ambiguous writes, the system SHALL remain fail-stopped, SHALL NOT restore the backup or start the predecessor, and SHALL retain the exact candidate, backup, quarantine, claim, and unresolved evidence. Manual finalization MAY start only the exact retained candidate after target-ledger, integrity, foreign-key, identity, and ownership verification; it SHALL not rerun the rehearsal or replay the backup.

A successful activation SHALL retain the verified native backup and its proof as transaction audit evidence. Any unresolved failure, including uncertain quarantine, process absence, persistence, identity, publication, or claim-release evidence, SHALL retain the backup and all evidence and SHALL keep the transaction fenced for explicit manual recovery. This feature-scoped retention SHALL NOT establish a project-wide backup-retention or disaster-recovery service.

The protected route SHALL reject ordinary launchd deployment, unqualified prepared or published sources, `latest` and other aliases, local or runtime-only sources, systemd or bare-Linux backends, another source or target ledger, a candidate/helper/configuration substitution, a version-only or highest-version admission, and any flag or manual action that bypasses the native backup, sequential rehearsal, startup quarantine, exclusivity proof, durable marker, exact identity, or manual-finalization boundary. Removing a marker, claim, quarantine, backup, or proof by pathname SHALL never authorize activation or recovery.

This exception SHALL NOT qualify schema-128-to-129 for another backend or deployment mode, authorize downgrade or generic rollback, or expand any existing migration-128 route pin.
### REQ-COMP-018 — Exact protected launchd schema-129-to-129 private refresh

THE SYSTEM SHALL guarantee one private native macOS launchd refresh route for a complete ordered source ledger equal to the exact migration-129 registry and a complete ordered target ledger equal to that same exact migration-129 registry. The route SHALL bind the complete registry bytes and digest, both complete ledger proofs, a dynamically selected full 40-character candidate SHA, the exact package version, an authenticated preparation-receipt digest, the separately selected controller commit and retained helper hash, the canonical database path, and the transaction identity as immutable per-transaction evidence. Candidate identity and receipt digest SHALL be selected and authenticated for each transaction; no fixed candidate pin, version, maximum ledger, unchanged-ledger assertion, or semantically equal registry SHALL substitute for that evidence.

The route SHALL require an independently authenticated receipt digest before admitting the candidate, and SHALL retain and re-verify the receipt, candidate, registry, controller, and helper evidence before backup, rehearsal, startup, recovery, or finalization. The controller and helper SHALL be separate independently recorded identities; the helper SHALL come from a clean retained controller checkout and SHALL NOT obtain candidate or registry bytes from an ambient controller working tree.

Before any candidate startup or live database write, the route SHALL reserve capacity for a native SQLite backup and one restore-capacity slot, quarantine the auto-loaded LaunchAgent plist, quiesce the predecessor, prove exclusive ownership of the database, WAL, and SHM, create and fsync a native SQLite backup-API snapshot, and verify its integrity, foreign keys, complete migration-129 ledger, logical contents, and transaction-bound proof. The exact candidate SHALL be run only against an isolated copy in the restore slot for a sequential rehearsal. The rehearsal SHALL be complete and verified before the live marker; it SHALL not run concurrently with backup creation or another copy and SHALL not consume unreserved capacity. The route SHALL perform no live migration: it SHALL not invoke schema migration or reconciliation against the live database, and the equal source and target ledgers SHALL not waive backup or rehearsal.

The route SHALL durably record `migration_started` immediately before starting the exact retained candidate. Before that marker, recovery MAY restore the exact captured predecessor runtime/configuration and restart only that predecessor after re-proving exclusive ownership, the exact migration-129 ledger, and unchanged logical contents; it SHALL not replay the backup to manufacture that proof. At or after the marker, including failed or interrupted startup, partial or ambiguous candidate writes, and uncertain publication, the transaction SHALL remain fail-stopped and candidate-only: it SHALL not restore the backup or start the predecessor. `finalize-current` SHALL be the only manual continuation and MAY start only the exact retained candidate after re-verifying the marker/phase, complete registry and ledger-129 proof, authenticated receipt digest, candidate and helper identity, integrity, foreign keys, configuration, ownership, and claim. The durable phase and immutable backup/rehearsal proofs are authoritative even when source and target ledgers are identical; ledger equality alone SHALL never authorize recovery or finalization.

A successful activation SHALL retain the verified native backup, rehearsal proof, receipt digest, registry/ledger proof, quarantine history, and identity evidence as transaction audit evidence. Any unresolved failure or uncertain claim, process, persistence, identity, publication, cleanup, or proof SHALL retain all available backup and evidence, the ownership claim, and the stopped fence for explicit recovery. The route SHALL reject ordinary launchd deployment, unqualified prepared or published sources, aliases, local or runtime-only sources, systemd or bare-Linux backends, another source or target ledger, candidate/helper/receipt/registry/configuration substitutions, and any flag or manual action that bypasses native backup, sequential rehearsal, exact per-transaction identity, durable marker, phase/proof authority, candidate-only finalization, or evidence retention. This exception SHALL NOT qualify schema-129-to-129 for another backend or deployment mode, authorize downgrade or generic rollback, or alter existing migration-128 and migration-128-to-129 route pins.

### REQ-COMP-019 — Exact ordinary launchd schema-129-to-131 transition

THE SYSTEM SHALL additionally qualify ordinary native launchd forward activation only from the complete ordered source ledger equal to the candidate registry prefix through migration 129 to the complete registry ending at migration 131 whose `crates/phoenix-db/src/migrations.rs` bytes have SHA-256 `850c25de411d57f14d07cd3fb09e3ee15e9e89f4fd07b24d48de00a533f1e1ac`.
The final three registry entries SHALL be 129 `single_conversation_name_and_route_projection`, 130 `continuation_handoff_candidates`, and 131 `request_capacity_diagnostics`. Missing, different, newer, reordered or name-mismatched evidence SHALL reject before disruption. The complete registry and digest SHALL remain immutable transaction evidence bound to the exact candidate and staged helper under `specs/launchd-deployment/requirements.md` REQ-LDD-028.

The candidate-start boundary, evidence retention, ownership claim and exact-candidate forward recovery of REQ-COMP-013 SHALL apply. After the first candidate startup attempt, partial migration130 commit, failed startup or uncertain health SHALL NOT authorize predecessor startup or snapshot replay. Explicit recovery MAY resume only the exact retained candidate with an exact complete source129, partial130 or target131 ledger. A source already equal to target131 SHALL remain ordinary deployment without pending migrations.

This qualification SHALL NOT require an ordinary standalone snapshot, claim protected-route backup or rehearsal proof, authorize a different source or registry, alter the exact123-to-128 ordinary interval, or alter the protected schema128-to-129 and private schema129-to-129 pins and obligations.
