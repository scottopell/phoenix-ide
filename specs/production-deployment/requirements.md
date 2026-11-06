# Cross-Platform Production Deployment Requirements

## Scope

Production operation on native macOS launchd, Linux systemd, and Linux without systemd. macOS uses ordinary local or published candidates with manual recovery under `specs/launchd-deployment/requirements.md`. Linux retains its ordinary runtime-artifact rollback and exact protected systemd contracts.

## Requirements

### REQ-PD-001 — Deterministic backend selection

When an operator invokes a production command, the system shall select launchd on macOS, systemd when Linux is running systemd, and the bare-Linux supervisor on Linux without systemd, and shall report the selected backend in status output.

### REQ-PD-002 — Explicit candidate sources

The local deployment command shall run required checks, build exact local `HEAD`, require the candidate to embed that complete 40-character lowercase commit SHA, and stage the resulting binary. The release deployment command shall resolve `latest` at most once or use the requested tag, bind the tag to one immutable commit, select the host target asset, verify the published checksum, and require the candidate's complete 40-character lowercase embedded SHA to equal the selected commit exactly; it shall not run repository checks, install dependencies, mutate the worktree, or compile.

A prepared-artifact source SHALL be admitted only on Linux x86_64 systemd under REQ-PD-029, using exact protected bytes and an independently verified clean controller without rebuilding on the deployment host. macOS SHALL admit only ordinary local or published candidates.

WHEN `latest` is requested,
THE release deployment command SHALL require the resolved tag and GitHub release metadata to identify a stable supported release.

WHEN an exact release-candidate tag is requested,
THE release deployment command SHALL require the tag, GitHub prerelease metadata, full embedded version, and exact commit identity to agree.

### REQ-PD-003 — Complete preparation before disruption

Before disrupting a running runtime, the system shall validate and stage its candidate, backend configuration, source-specific environment snapshot, runtime identity, endpoint, artifact hashes, independent activation program, immutable handoff and initial durable status. Linux shall also prepare its specified rollback inputs and destination-space reservations. Protected systemd activation under REQ-PD-023 or REQ-PD-029 shall preserve captured installed environment, service and socket configuration.

### REQ-PD-004 — Backend-owned activation

Before stopping Phoenix, the system shall transfer activation to an owner independent of Phoenix and the initiating shell: a one-shot LaunchAgent for launchd, a root-owned transient unit for systemd, or the persistent same-user supervisor for bare Linux.

### REQ-PD-005 — Immutable, secret-safe handoff

The activation owner shall consume immutable host-resident inputs identified by hashes. Manifests, status and process arguments shall omit environment values and other secrets. Candidate migration diagnostics shall flow to the transaction log without a separate diagnostic transformation layer.

### REQ-PD-006 — Single fenced activation writer

While an activation or unresolved transaction owns the host deployment claim, the system shall reject another deployment without changing production state. Claim acquisition and release shall be serialized, and an owner shall release a claim only when its matching terminal status is durable; an older owner shall not clear a newer claim.

### REQ-PD-007 — Validated privilege boundary

The systemd handoff shall copy inputs into a root-owned transaction location before activation. The privileged path shall validate protocol version, ownership, restrictive modes, hashes, allowed unit names, service user, data location, and fixed installation roots, and shall reject symlink traversal or arbitrary user-provided targets.

### REQ-PD-008 — Atomic installation

When installing a candidate or restoring authorized Linux rollback state, the activation owner shall fsync staged data and atomically replace the target without first unlinking the live path. Linux candidate and rollback copies shall be reserved on their destination filesystems before disruption. macOS plist publication follows REQ-LDD-005.

### REQ-PD-009 — Exact runtime verification

A deployment shall commit only after observing a new backend-owned runtime process and verifying that its credential-free version endpoint reports the exact expected package version and complete 40-character lowercase embedded git SHA. The candidate SHA shall equal the transaction's selected full source commit exactly; prefix equality shall not establish candidate identity. Bare Linux shall additionally bind verification to the supervisor's direct child PID and `/proc` start time.

### REQ-PD-010 — Linux runtime-artifact rollback

For ordinary Linux activation failure after disruption, the owner shall stop the candidate, atomically restore the previous binary, backend configuration, environment and service state, verify the captured predecessor identity at its previous endpoint, restore its deployed SHA, and distinguish successful runtime-artifact rollback from rollback failure. A captured installed predecessor may report a legacy 12-character lowercase SHA only in this rollback role; candidates require a complete 40-character identity. Runtime-artifact rollback shall not restore a database or establish compatibility with candidate-mutated data. Protected systemd activation follows the stricter pre-marker unchanged-predecessor and post-marker candidate-only boundary in REQ-PD-025. macOS shall instead follow REQ-LDD-030 and shall never automatically start the predecessor after disruption.

### REQ-PD-011 — Truthful durable status and recovery

After exact verification, the system shall write `deployed.sha` from the selected candidate's embedded source commit and durably persist `committed` before releasing the claim. A nonterminal or interrupted transaction shall remain visible and actionable and shall never be inferred as successful solely from a PID, active unit, responsive port, or installed file.

### REQ-PD-012 — Configuration source of truth

Ordinary operator deployment shall load `.phoenix-ide.env` once and use that snapshot for preflight, installation and endpoint selection. An in-app source-pinned release controller shall preserve the installed launchd environment and PATH instead of applying ambient checkout configuration. Status shall inspect installed configuration. Linux rollback shall use the captured previous installed snapshot. Protected systemd activation shall preserve captured installed environment, service, socket and canonical database path. Modern operation shall ignore legacy launchd override stores, systemd drop-ins and inferred detached-daemon environments.

### REQ-PD-013 — Consistent operator surface

Each backend shall support `prod deploy`, `prod deploy --release TAG|latest`, `prod status`, and `prod stop`. Positional deployment versions shall be rejected with migration guidance. `prod set` and `prod unset` shall reject configuration mutation and direct the operator to edit `.phoenix-ide.env`; they shall not create backend-specific override state.

### REQ-PD-014 — Persistent bare-Linux ownership

On bare Linux, an owner-only supervisor shall directly parent Phoenix, authenticate clients through an owner-only Unix socket and Linux peer credentials, accept only a transaction ID plus manifest hash, and reject stale, replayed, concurrent, malformed, or tampered handoffs. `prod stop` shall stop the managed child without stopping the supervisor.

WHEN a bare supervisor is already running
AND the startup-bound digest reported by that running supervisor is not byte-for-byte the selected supervisor artifact digest
THE SYSTEM SHALL refuse deployment before disrupting production
EVEN WHEN both supervisors report the same protocol version.

THE SYSTEM SHALL NOT replace, restart, or reuse a changed running supervisor as a general controller-compatibility mechanism.

### REQ-PD-015 — Honest bare-Linux reboot persistence

Bare-Linux installation shall start the supervisor independently for the active boot and install an owner `@reboot` entry when a compatible crontab exists. Otherwise it shall report exact host rc guidance and shall not claim reboot persistence.

### REQ-PD-016 — Disposable integration safety

Integration harnesses shall use randomized virtual machines or containers, units, labels, paths, ports, databases, and transaction identities; shall structurally refuse production resources; and shall not mount production data, host credentials, or the host worktree into authoritative activation tests.

### REQ-PD-017 — Supported published Linux assets

Published releases shall provide checksummed `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` binaries in addition to supported macOS assets, and release candidate selection shall reject an asset whose target, checksum, version, or embedded commit does not match the selected release.

### REQ-PD-018 — Withdrawn macOS deployment route

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-230.

### REQ-PD-019 — Withdrawn macOS deployment route

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-230.

### REQ-PD-020 — Withdrawn macOS deployment route

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-230.

### REQ-PD-021 — Explicit protected Linux mode selection

The exact Linux prepared source shall require `--systemd-current-schema-upgrade` and the receipt, provenance and lifecycle contract in REQ-PD-029. macOS shall reject protected/prepared deployment modes and use only the ordinary candidate-owned migration path.

### REQ-PD-022 — Withdrawn macOS deployment route

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-230.

### REQ-PD-023 — Exact protected systemd published-release admission

WHEN the explicit `--systemd-current-schema-upgrade` published-release mode is selected,
THE SYSTEM SHALL admit only Linux x86_64 systemd activation with all of these immutable candidate identities:

| Evidence | Exact value |
| --- | --- |
| Published tag | `v0.13.0-rc.4` |
| Full candidate commit | `3f0ba4602faa71ebad5088e1ffc856f4c7d511d9` |
| Embedded version | `0.13.0-rc.4` |
| Asset | `phoenix_ide-x86_64-unknown-linux-musl` |
| Asset SHA-256 | `bcb74d565c828843596e5109ea9034a2b0e505aab4ba6c18c5cca8e116f5677e` |
| Candidate `crates/phoenix-db/src/migrations.rs` SHA-256 | `700162dd8e41305546cf2acd2763bad5603c642840f5be851fb788353879a36a` |
| Target | Complete ordered registry from those exact source bytes, ending at migration 128 |

THE SYSTEM SHALL require the source database's complete ordered `(version, name)` ledger to equal that registry's exact prefix through migration 65, not merely a maximum version or count. It SHALL bind and validate the complete registry and all candidate evidence before disruption, and re-verify retained evidence before migration, activation, or recovery.

THE SYSTEM SHALL require a separately explicit full controller source commit different from the candidate commit and a retained helper hash verified against that controller source. It SHALL bind both identities independently of the release identity. It SHALL require an existing installed predecessor and preserve the installed environment, service unit, socket unit, and canonical `/var/lib/phoenix-ide/prod.db` path without configuration replacement from ambient files.

During root staging, before installing a start inhibit, stopping either unit, or writing a migration marker, THE SYSTEM SHALL derive and bind both retained predecessor and candidate health URLs from the preserved socket and effective installed environment. The admitted socket shape SHALL contain exactly one TCP `ListenStream` in one `[Socket]` section: a decimal port, a wildcard IPv4 or bracketed IPv6 address plus port, or an explicit loopback IPv4 or bracketed IPv6 address plus port. Within `[Socket]`, only the case-sensitive options `ListenStream`, `NoDelay`, `Backlog`, and `BindIPv6Only` SHALL be admitted; `Accept`, `BindToDevice`, and every other option SHALL be rejected. Only the exact section names `[Unit]`, `[Socket]`, and `[Install]` SHALL be admitted, without repeated sections. A bare port SHALL select IPv6 loopback; wildcard addresses SHALL select loopback in the same address family; an explicit loopback address SHALL be preserved. The port SHALL be in 1–65535. Unix sockets, hostnames, non-loopback specific addresses, multiple or other listener types, port ranges, and unsupported syntax SHALL be rejected. If `PHOENIX_PORT` is present, it SHALL be a decimal port equal to the socket listener port. IPv6 URL hosts SHALL be bracketed.

The retained URL scheme SHALL use the pinned candidate's `ConfigSource::from_env` TLS interpretation: presence of both certificate and key paths selects manual HTTPS regardless of mode; presence of only one path rejects; without either path, trimmed case-insensitive `PHOENIX_TLS` values empty/`0`/`false`/`off`/`none` select HTTP and `1`/`true`/`on`/`auto` select automatic HTTPS. `manual` without both paths and every other mode SHALL reject. Manual certificate/key paths SHALL be absolute regular files without symlink or parent-directory (`..`) traversal, validated during root staging before any disruption or marker. The service user's uid and primary plus supplementary groups SHALL be resolved; conservative Unix owner/group/other mode checks SHALL require execute traversal on every ancestor directory and read permission on each file, with root's permission bypass admitted. A missing identity, uninspectable path, or denied mode SHALL reject before disruption.

`PHOENIX_TLS_CA_CERT_PATH` SHALL be interpreted only with a complete manual certificate/key pair; otherwise it SHALL have no TLS effect. If present with that pair, it SHALL meet the same absolute regular non-symlink, traversal and service-readability checks. Before staging completes or disruption begins, Python `ssl.SSLContext.load_cert_chain` SHALL prove certificate/key parsing and matching, and `load_verify_locations` SHALL parse the configured CA. The CA SHALL be exactly one PEM certificate within 32 KiB, with Basic Constraints declaring CA capability and, if Key Usage is present, certificate-signing permission. An explicitly resolved and successfully probed `openssl` executable SHALL verify those capabilities and the server chain against only that CA using `verify -trusted CA -partial_chain -untrusted CERT -purpose sslserver CERT`. The configured certificate file SHALL supply the leaf and any intermediate certificates; system trust roots SHALL NOT substitute for the configured CA. No alternate CA/configuration authority SHALL be persisted. Invalid, unrelated, missing or unreadable CA evidence SHALL reject before inhibit, stop or live marker.

THE SYSTEM SHALL reject this mode for `latest`, other releases, other source ledgers or targets, local/prepared sources, launchd, and bare Linux. It SHALL NOT offer a generic baseline/target selection mechanism or infer future release support from this qualification.

### REQ-PD-024 — Offline pristine backup and rehearsal before live migration

For the protected systemd transition, THE SYSTEM SHALL validate all immutable source, controller/helper, configuration, predecessor and candidate evidence before disruption. It SHALL resolve a regular executable `lsof` to its exact absolute path and successfully probe it before installing either inhibit or stopping either unit; all database exclusivity checks SHALL use that preflighted executable, not another PATH lookup. Missing or unusable `lsof` SHALL fail as a precondition with no inhibit, stop or retained claim. Before durable `activating`, it SHALL capture and fsync the transaction-bound predecessor service/socket active and enabled state for interruption recovery.

Before stopping either unit, it SHALL install and fsync transaction-bound persistent systemd start-inhibit drop-ins for both service and socket, reload systemd, and verify the files and effective loaded configuration. The inhibit SHALL prevent manual, dependency/socket and reboot activation throughout disrupted pre-marker and unresolved post-marker states. Missing, altered, unsafe or foreign inhibit evidence SHALL fail closed, not authorize startup or blind removal. It SHALL stop both service and socket, prove both inactive and the canonical database exclusively owned, and re-verify the exact stopped source ledger and logical contents. It SHALL create, fsync, integrity-check, and bind a persistent pristine SQLite backup and its hash to that transaction before invoking the candidate on the live database. The stopped snapshot SHALL include legitimate predecessor writes made after preflight and before shutdown. Once that snapshot succeeds, its exact stopped-source evidence SHALL replace preflight evidence as the unchanged-source comparison authority for pre-marker recovery; any subsequent ledger or logical-content change SHALL forbid predecessor restart.

THE SYSTEM SHALL run the exact candidate's `--migrate-only` operation against an isolated copy of that backup, not against the pristine backup or live database, and durably verify successful exit, exact complete target ledger, SQLite integrity and foreign-key checks, and unchanged pristine backup before permitting the live irreversible marker. Rehearsal proof SHALL bind the candidate, registry, backup and transaction evidence and SHALL be re-verified before live invocation or post-marker recovery.

Backup or rehearsal capacity failure, failed rehearsal, or failed verification SHALL remain pre-marker failures and MAY recover only the verified unchanged predecessor under REQ-PD-025. This systemd contract SHALL NOT impose a 20 GiB free-space floor. The exact pristine backup and its proof SHALL remain after success; the rehearsal copy SHALL NOT replace them.

### REQ-PD-025 — Durable irreversible marker and exact-candidate-only recovery

For protected systemd activation, THE SYSTEM SHALL durably write transaction-bound `migration_started` immediately before the first live candidate `--migrate-only` invocation, after successful backup/rehearsal verification. Durable status SHALL remain `activating` across this boundary, with the marker file and `migration_started` boolean recording it; `migration_started` SHALL NOT be a separate transaction state. Before that marker, recovery MAY restore captured runtime artifacts and resume the predecessor only after proving exclusive offline ownership, the exact source ledger, captured predecessor/configuration identity, and equality to stopped-snapshot logical contents if snapshot evidence exists; it SHALL NOT replay the backup to manufacture unchanged-source proof. The inhibit MAY be removed only after that pre-marker unchanged-predecessor runtime restoration is authorized and completed, before predecessor restart, or immediately before verified exact-target candidate startup. Removal SHALL verify the transaction-bound inhibit and effective configuration, durably remove only those drop-ins, reload systemd and verify the resulting preserved configuration. Unproven or failed pre-marker rollback SHALL retain the claim/inhibit and report `activation_failed_rollback_failed`, not the ordinary-mode failure state.

At or after that marker, including failed invocation, interruption, partial commits, or uncertain writes, THE SYSTEM SHALL NOT restore any database backup, restore predecessor runtime artifacts, or start the old binary. Failure SHALL attempt to stop service and socket, retain the exact candidate, claim, backup and immutable evidence, and report forward-only recovery rather than rollback success.

Explicit `prod recover-systemd-current TXN` (the retained helper's `recover-current` action) SHALL select the post-marker path by marker presence, including unsafe marker evidence, rather than by a status state named `migration_started`. An absent marker SHALL select only explicit pre-marker recovery for the exact active protected transaction with durable `activating` or `activation_failed_rollback_failed` status, `migration_started=false`, intact transaction-bound inhibits and captured predecessor unit state. It SHALL validate the retained manifest/controller/helper/rollback artifacts and configuration, stop both units, prove inactivity, preflight `lsof` after stop, and prove exclusive live database integrity/foreign keys and exact source-prefix65.

If no stopped snapshot was captured, normal predecessor data writes before shutdown SHALL be admitted without comparing against a stale preflight logical digest. If snapshot evidence is present, intact snapshot proof and equality of the live logical contents SHALL be required; partial or malformed snapshot evidence SHALL NOT authorize restore. Rehearsal evidence SHALL NOT authorize database replay.

It SHALL restore and verify only the captured predecessor binary/configuration before inhibit removal, then restore and verify the captured active/enabled unit state and applicable predecessor health identity before durable `activation_failed_rolled_back` and exact claim release. Failure SHALL retain the fence as `activation_failed_rollback_failed`; unavailable `lsof` SHALL leave both units stopped.

Untouched prepared/no-disruption transactions SHALL refuse recovery and use the prepared abandonment path. A crash before predecessor-state publication SHALL leave durable `prepared` ownership with no inhibit/stop/marker, eligible for validated prepared abandonment, not guessed restoration. Ambiguous, missing or malformed disrupted evidence SHALL remain fenced. It SHALL never replay a snapshot. Marker absence SHALL NOT authorize forward migration.

On the selected post-marker path, recovery SHALL stop both service and socket and prove inactivity before detailed post-marker manifest, inhibit, marker, evidence or database validation; validation failure SHALL NOT leave an old running service/socket untouched. It SHALL preflight the required `lsof` executable only after both-unit stop, fail stopped if it is unavailable or unusable, then prove exclusive database ownership and use only the transaction's exact retained candidate/controller/helper, matching claim and valid marker, intact backup/rehearsal proofs and preserved configuration. It MAY rerun that candidate's `--migrate-only` only when the entire ledger equals a prefix of the same exact registry from migration 65 through 128; a different, missing, renamed, reordered or newer entry SHALL fail closed. After successful detailed retained-evidence and stopped-ledger admission, recovery SHALL durably write status `activating` with `migration_started=true` before either a partial-prefix migration rerun or exact-target candidate startup authorization. An exact-target ledger MAY proceed to candidate finalization without rerunning migration. Recovery SHALL NOT substitute another candidate, helper or configuration.

Activation or recovery SHALL verify the exact complete target ledger, SQLite integrity and foreign-key checks before candidate startup, then verify the exact backend-owned candidate identity before publishing `deployed.sha`, durable `committed`, and matching claim release. Unresolved recovery SHALL retain the fence; success SHALL retain the pristine backup and evidence.

### REQ-PD-026 — Ordinary systemd pending migrations fail before handoff

For ordinary systemd deployment through a controller enforcing this contract, WHEN an existing production database is present, preflight SHALL derive the target ledger by executing the exact candidate's `--migrate-only` on an isolated empty database with isolated HOME and DB path, not by parsing a migration registry from source. It SHALL verify successful exit, target integrity/foreign keys, a nonempty sorted unique complete `(version, name)` ledger, and unchanged candidate hash across derivation. WHEN the existing production database fails integrity/foreign-key checks or its complete ledger differs from that derived target (including pending migrations), THE SYSTEM SHALL reject before activation handoff or runtime disruption. Failure to derive or verify the target SHALL fail closed.

If the configured database path is missing and any installed predecessor binary, environment, service/socket unit, deployed identity artifact or runtime identity exists, THE SYSTEM SHALL reject before handoff; an unavailable mount or unlinked database held open by the predecessor SHALL NOT be treated as a first install. The helper SHALL mirror this gate when a predecessor or captured rollback artifacts exist. Only a true first install with no predecessor evidence MAY omit the database. It SHALL NOT silently route that interval through ordinary runtime-artifact rollback. Exact-target/no-pending ordinary activation remains supported. This guarantee SHALL NOT be attributed to historical controllers without this preflight gate and SHALL NOT expand to launchd or bare Linux. This controller-side preflight SHALL leave the protocol-v1 ordinary helper manifest unchanged; it SHALL NOT require a registry field or retrofit the protected protocol into old helpers.

### REQ-PD-027 — Retire only positively identified partial systemd staging

THE SYSTEM SHALL serialize systemd staging, activation and recovery with the global deployment operation lock, and persist matching `staging` status with `migration_started = false` before publishing the staging claim. Under that lock, it MAY retire an interrupted partial staging transaction only with exact staging-status shape, matching owner/claim (or no claim), and absent irreversible marker, before any prepared handoff. Retirement SHALL remove only that transaction's partial inputs and matching staging ownership, fsync affected directories, and leave production unchanged.

Ambiguous evidence, a prepared transaction, a post-marker transaction, or an active competing operation SHALL remain fenced rather than be treated as abandoned staging. Missing or unreadable status SHALL NOT authorize clearing an extant claim.

### REQ-PD-028 — Withdrawn macOS deployment route

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-230.

### REQ-PD-029: Exact prepared Linux systemd candidate

WHEN an operator explicitly selects `--systemd-current-schema-upgrade` with `--prepared-artifact DIR`, exact expected candidate `237100c79d9ce672867b284167a099fa0b7a2bcf`, and a separately selected different controller source commit,
THE SYSTEM SHALL admit only a Linux `x86_64-unknown-linux-musl` prepared bundle produced by operation `prepare-incident-237100-linux-x86_64` from that exact controller commit on `refs/heads/main`,
AND SHALL require exactly the standalone payload `phoenix_ide-x86_64-unknown-linux-musl-prepared-237100c79d9c` and receipt `LINUX-PREPARATION-RECEIPT-x86_64-unknown-linux-musl.json`, with no symlinks or additional entries.

The receipt SHALL have one closed schema that binds schema version 1, the exact operation, target, embedded version `0.13.0-rc.4`, full candidate commit, separately different full controller commit, complete registry128 source digest `a4eb047e59b21997ea068f1bd5e97c9c943f74a4c5670cdd8ff8ac10ffd5eee6`, positive workflow run and attempt identifiers, `workflow_dispatch`, `refs/heads/main`, the sole payload basename and SHA-256, and verified clean-source, static-ELF and embedded-identity checks. The controller SHALL require the receipt controller commit to equal the separately selected controller source commit, independently verify the candidate's full embedded identity and payload hash, materialize the migration registry from the exact candidate commit and the helper from the exact controller commit, and retain/hash-bind the receipt, candidate, registry and helper as distinct transaction artifacts. For this prepared-systemd variant, `--expected-preparation-receipt-sha256` SHALL be mandatory, SHALL accept only the exact lowercase receipt-byte SHA-256 supplied from independently authenticated protected GitHub run/source/artifact evidence rather than derived from the local artifact directory, and SHALL be rejected for every other source or deployment route. Before parsing receipt fields, trusting its payload hash, invoking sudo or mutating the host, the controller SHALL require the receipt bytes copied from the anchored open descriptor to equal that supplied digest. Local receipt JSON alone SHALL NOT establish the protected run/source/artifact association.

The privileged helper SHALL re-parse the same closed receipt, reject prepared source evidence in ordinary or published-release modes, reject published-release fields in prepared mode, and cross-check exact candidate, controller, registry, payload and retained-receipt hashes before disruption. Published protected systemd protocol version 2 SHALL remain unchanged; the prepared protected handoff SHALL use protocol version 3. The exact published source in REQ-PD-023 and its pins SHALL remain valid and unchanged; prepared evidence SHALL NOT be represented as a published release.

This exact prepared source SHALL reuse the protected systemd lifecycle in REQ-PD-024 and recovery in REQ-PD-025 without weakening them: persistent verified service/socket inhibits, both-unit stop, exclusive offline ownership, pristine backup, isolated exact-candidate rehearsal, durable live `migration_started` boundary, exact-target verification, candidate-only post-marker recovery, and retained backup/evidence SHALL all remain required. It SHALL admit only the complete ordered source ledger equal to the exact candidate-registry prefix through migration 65 and the exact target128. No other prepared candidate, architecture, source baseline, target, backend, same-schema replacement, downgrade or generic rollback guarantee follows from this qualification.

### REQ-PD-030 — Withdrawn macOS deployment route

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-230.

### REQ-PD-031 — macOS manual-recovery contract

Native macOS deployment shall follow REQ-LDD-029–032: invoke candidate-owned migration once, retain failed ownership and diagnostics after disruption, require operator recovery, and provide no automatic database rollback or transaction resumption. This contract shall not alter Linux deployment, recovery or rollback.
