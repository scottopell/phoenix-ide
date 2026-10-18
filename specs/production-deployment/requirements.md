# Cross-Platform Production Deployment Requirements

## Scope

Safe replacement and operation of native Phoenix production runtimes on macOS launchd, Linux systemd, and Linux hosts without systemd. The deployment source is exact checked local `HEAD`, an immutable published release, or the explicit launchd-only prepared-artifact paired source under REQ-PD-018. The protected systemd published-release exception is limited to REQ-PD-023–027. This specification owns shared cross-platform guarantees and the release assets required to satisfy them. `specs/launchd-deployment/requirements.md` cumulatively refines launchd-specific mechanics; it does not redefine the shared guarantees.

## Requirements

### REQ-PD-001 — Deterministic backend selection

When an operator invokes a production command, the system shall select launchd on macOS, systemd when Linux is running systemd, and the bare-Linux supervisor on Linux without systemd, and shall report the selected backend in status output.

### REQ-PD-002 — Explicit candidate sources

The local deployment command shall run required checks, build exact local `HEAD`, require the candidate to embed that complete 40-character lowercase commit SHA, and stage the resulting binary. The release deployment command shall resolve `latest` at most once or use the requested tag, bind the tag to one immutable commit, select the host target asset, verify the published checksum, and require the candidate's complete 40-character lowercase embedded SHA to equal the selected commit exactly; it shall not run repository checks, install dependencies, mutate the worktree, or compile.

The prepared-artifact source SHALL be admitted only on macOS launchd under REQ-PD-018, using exact protected prepared bytes and an independently verified clean controller without rebuilding or ad-hoc resigning.

WHEN `latest` is requested,
THE release deployment command SHALL require the resolved tag and GitHub release metadata to identify a stable supported release.

WHEN an exact release-candidate tag is requested,
THE release deployment command SHALL require the tag, GitHub prerelease metadata, full embedded version, and exact commit identity to agree.

### REQ-PD-003 — Complete preparation before disruption

For the prepared-artifact paired source under REQ-PD-018, the captured installed launchd plist environment and PATH SHALL be the sole configuration authority; `.phoenix-ide.env` SHALL NOT replace credentials, models, paths or network configuration. For protected systemd published-release activation under REQ-PD-023, the captured installed environment, service and socket SHALL be preserved instead; `.phoenix-ide.env` SHALL NOT replace them. Protected published-release current-schema activation under REQ-PD-021 SHALL preserve the captured installed launchd configuration and database path. The ordinary environment-snapshot rules below apply only to unprotected local/published sources.

Before disrupting a running runtime, the system shall stage and validate the candidate, backend configuration, source-specific environment snapshot, rollback inputs, runtime identities and endpoints, destination-space reservations, artifact hashes, backend activation program, immutable handoff, and initial durable transaction status.

### REQ-PD-004 — Backend-owned activation

Before stopping Phoenix, the system shall transfer activation to an owner independent of Phoenix and the initiating shell: a one-shot LaunchAgent for launchd, a root-owned transient unit for systemd, or the persistent same-user supervisor for bare Linux.

### REQ-PD-005 — Immutable, secret-safe handoff

The activation owner shall consume only immutable host-resident transaction inputs identified by hashes. Manifests, statuses, logs, process arguments, and diagnostics shall omit environment values and other secrets.

### REQ-PD-006 — Single fenced activation writer

While an activation or unresolved transaction owns the host deployment claim, the system shall reject another deployment without changing production state. Claim acquisition and release shall be serialized, and an owner shall release a claim only when its matching terminal status is durable; an older owner shall not clear a newer claim.

### REQ-PD-007 — Validated privilege boundary

The systemd handoff shall copy inputs into a root-owned transaction location before activation. The privileged path shall validate protocol version, ownership, restrictive modes, hashes, allowed unit names, service user, data location, and fixed installation roots, and shall reject symlink traversal or arbitrary user-provided targets.

### REQ-PD-008 — Atomic installation

When installing a candidate or restoring rollback state, the activation owner shall fsync staged data and atomically replace each target without first unlinking the live path. Candidate and rollback copies shall be reserved on each destination filesystem before disruption.

### REQ-PD-009 — Exact runtime verification

A deployment shall commit only after observing a new backend-owned runtime process and verifying that its credential-free version endpoint reports the exact expected package version and complete 40-character lowercase embedded git SHA. The candidate SHA shall equal the transaction's selected full source commit exactly; prefix equality shall not establish candidate identity. Bare Linux shall additionally bind verification to the supervisor's direct child PID and `/proc` start time.

### REQ-PD-010 — Verified rollback

Protected current-schema launchd transactions SHALL obey the pre-invocation-only predecessor recovery boundary in REQ-LDD-021–022. Qualified ordinary launchd migration activation SHALL obey REQ-LDD-023's candidate-start boundary; the runtime-artifact rollback rules below SHALL NOT override either boundary.

For prepared-artifact paired transactions, predecessor startup/rollback is conditional on the verified recovery authorization under REQ-LDD-017. If database, ownership, snapshot or captured runtime/configuration proof fails, the system SHALL instead attempt teardown, retain a stopped service and unresolved claim, and report recovery failure; it SHALL NOT bootstrap an unproven predecessor. Protected systemd activation follows the stricter pre-marker unchanged-predecessor and post-marker candidate-only boundary in REQ-PD-025. The artifact-rollback attempt below applies to other sources subject to their explicitly qualified compatibility boundaries.

If activation fails after disruption, the activation owner shall stop the candidate, atomically restore the previous binary, backend configuration, environment snapshot, and service state, verify the captured previous runtime identity at its previous endpoint, restore the previous deployed SHA, and durably distinguish successful runtime-artifact rollback from rollback failure.

Only in this rollback role, the captured identity of an already-installed previous runtime may contain either a legacy 12-character lowercase git SHA or a full 40-character lowercase git SHA. This allowance shall not admit a 12-character identity for a candidate, controller, release asset, newly installed runtime, or general downgrade path, and shall not establish cross-version deployment compatibility.

Ordinary deployment rollback shall restore runtime artifacts only and shall not guarantee that the predecessor can use a database changed by the candidate. The explicit prepared-artifact ProductConversation upgrade shall restore its verified predecessor database, binary, and configuration before restarting the predecessor; its boundary is defined by REQ-PD-018 and `specs/launchd-deployment/requirements.md`.

### REQ-PD-011 — Truthful durable status and recovery

After exact verification, the system shall write `deployed.sha` from the selected candidate's embedded source commit and durably persist `committed` before releasing the claim. A nonterminal or interrupted transaction shall remain visible and actionable and shall never be inferred as successful solely from a PID, active unit, responsive port, or installed file.

### REQ-PD-012 — Configuration source of truth

For the prepared-artifact paired source under REQ-PD-018, the captured installed launchd plist environment and PATH SHALL be the sole configuration authority; `.phoenix-ide.env` SHALL NOT replace credentials, models, paths or network configuration. For protected systemd published-release activation under REQ-PD-023, the captured installed environment, service and socket SHALL be preserved instead; `.phoenix-ide.env` SHALL NOT replace them. Protected published-release current-schema activation under REQ-PD-021 SHALL preserve the captured installed launchd configuration and database path. The ordinary environment-snapshot rules below apply only to unprotected local/published sources.

Ordinary local/published deployment outside the protected modes above shall load `.phoenix-ide.env` once and use that exact snapshot for preflight, installation, and candidate endpoint selection. Runtime status shall inspect installed configuration, rollback shall use the previous installed snapshot, and modern operation shall ignore rather than migrate or consult a legacy launchd override store, systemd drop-in, or inferred detached-daemon environment.

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

### REQ-PD-018 — Prepared-artifact paired launchd boundary

WHEN the explicit prepared-artifact ProductConversation upgrade is selected
THE SYSTEM SHALL bind the protected candidate provenance to a clean controller helper and private transaction proof
AND SHALL fail closed when database ownership, legacy ledger/table shape, snapshot integrity, configuration identity, or rollback proof cannot be established

THE SYSTEM SHALL NOT generalize this feature-scoped pair into automatic database rollback for other deployment sources or backends.

### REQ-PD-019 — Feature-scoped prepared paired upgrade

THE shared production deployment contract SHALL treat the macOS launchd prepared-artifact/ProductConversation database upgrade as an explicit feature-scoped guarantee, not as a general snapshot or downgrade facility. The separate systemd exception in REQ-PD-023–027 does not inherit the launchd paired rollback guarantee. Other deployment paths retain their separately specified database behavior.

### REQ-PD-020 — Narrow committed paired finalization

The shared command surface SHALL expose `prod finalize-paired TXN` only for the launchd paired contract under `specs/launchd-deployment/requirements.md` REQ-LDD-018. This operation SHALL be publication/reservation-only and SHALL NOT reuse runtime activation or database rollback. Unknown ownership, helper absence, committed identity or captured configuration SHALL retain the fence and refuse.

### REQ-PD-021: Explicit protected database mode selection

**Condition:** When an operator deploys a prepared artifact through the production interface,
**System:** the interface SHALL require exactly one of paired legacy upgrade or current-schema upgrade, SHALL reject local sources and release sources except for the protected current-schema admission below, SHALL require the expected full candidate commit, and SHALL expose mode-matched finalization and recovery commands without weakening prepared-artifact signature, notarization, receipt, architecture, source-commit, or embedded-identity verification.

**Rationale:** Artifact provenance and database compatibility are separate dimensions; selecting a newer database family must not bypass the prepared artifact's existing supply-chain contract.

**Condition:** When an operator explicitly selects a published release for protected current-schema launchd activation,
**System:** the interface SHALL require the exact immutable release identity and a separately explicit full controller commit for the retained activation helper; SHALL retain the helper's hash and obtain the candidate binary and complete migration registry from the release commit, not the controller commit; SHALL admit only the source ledger exactly equal to that registry's complete prefix through 114 and the exact qualified target 128 under REQ-LDD-019; and SHALL reuse the protected snapshot, capacity, exclusivity, durable `migration_started`, and candidate-only post-invocation recovery/finalization lifecycle. It SHALL reject aliases, other source baselines, target registries, and backends before disruption. Prepared exact-123/exact-target and ordinary exact-123-to-128 admissions SHALL remain unchanged; this exception SHALL NOT establish generic release, range, or rollback compatibility.

For prepared current-schema launchd candidate `23badb8be4bc8d05a7e1c49c5367ceb14ebccd92` only, the unchanged public command `./dev.py prod deploy --prepared-artifact DIR --expected-full-commit 23badb8be4bc8d05a7e1c49c5367ceb14ebccd92 --current-schema-upgrade` SHALL automatically emit the typed `prepared_source_baseline: 114` and `candidate_registry_expected_sha256: a4eb047e59b21997ea068f1bd5e97c9c943f74a4c5670cdd8ff8ac10ffd5eee6` evidence defined by REQ-LDD-019. Receipt/signature/version/embedded-SHA checks SHALL remain intact; the complete registry SHALL be obtained by `git show` from the exact locally present candidate commit and hash-verified, independently of the different clean controller checkout supplying the retained hash-bound helper. This admits only exact prefix114 or exact target128 to that exact registry128 and preserves the protected lifecycle, proof tamper rejection and candidate-only boundary. Another receipt commit SHALL NOT receive this qualification. Generic prepared123/target128, published-release114 and ordinary paths SHALL remain unchanged; this is explicit candidate/source-kind expansion with RC4 registry semantics unchanged, not generic prepared114 support.

### REQ-PD-022: Ordinary forward-only recovery surface

When ordinary launchd deployment has pending migrations outside an explicitly qualified ordinary interval, the interface SHALL reject deployment before runtime disruption, not fall back to unqualified runtime-artifact rollback. Protected exact-114-to-128 release admission SHALL NOT qualify that interval for ordinary deployment.

**Condition:** When ordinary launchd activation crosses the qualified candidate-start boundary and does not verify candidate health,
**System:** the production interface SHALL report that predecessor rollback is forbidden, retain the deployment claim and exact candidate evidence, and expose only explicit `prod resume-forward TXN` recovery for that transaction. It SHALL NOT require or imply a standalone backup for the normal in-place route.

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

Backup or rehearsal capacity failure, failed rehearsal, or failed verification SHALL remain pre-marker failures and MAY recover only the verified unchanged predecessor under REQ-PD-025. This systemd contract SHALL NOT impose the launchd capacity policy or its 20 GiB free-space floor. The exact pristine backup and its proof SHALL remain after success; the rehearsal copy SHALL NOT replace them.

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

### REQ-PD-028 — Explicit exact current-schema launchd forward replacement

WHEN an operator selects `prod repair-current TX --prepared-artifact DIR --expected-full-commit REPAIR_SHA`, THE SYSTEM SHALL restrict the operation to the original/repair qualification in `specs/compatibility/requirements.md` REQ-COMP-015 and the sealed child protocol in `specs/launchd-deployment/requirements.md` REQ-LDD-025. It SHALL preserve prepared-artifact supply-chain and full-identity checks, exact captured parent configuration and database, the original claim and immutable private parent audit evidence, independently clean controller/helper provenance, and immutable candidate git-object registry evidence. After full admission and before child sealing, it SHALL physically fence the installed original runtime with the exact stopped binary/plist boundary in REQ-LDD-025 so historical helpers and login/reboot persistence cannot start it. It SHALL NOT reuse ordinary deploy, alter the original transaction evidence, acquire a child claim, infer a schema range, or permit an original/predecessor start or snapshot replay.

WHEN `prod resume-current-repair TX` is selected, THE SYSTEM SHALL use only the same retained sealed child, not ambient candidate/configuration/helper/registry inputs. Durable reconciliation and target/start/failure/finalization checkpoints SHALL make interruption fail closed and exact-child resumption idempotent. Status and operation refusal SHALL distinguish pending or malformed repair evidence from terminal child success while leaving parent committed/pending status unchanged by the repair. The child SHALL seal a private byte-exact `parent-status.json` snapshot and hash, require live global status equality until terminal commit, and use that snapshot for terminal historical audit after another deployment overwrites global status. Lightweight classification SHALL check exact stored shapes/paths and parent evidence hashes without database/backup scans; full backup/proof and exclusive offline live-database validation SHALL remain operation admission requirements before runtime mutation. Repair identity and health, durable publication and owned cleanup, then terminal child status SHALL precede release/fsync of only the exact parent claim; any ambiguity SHALL retain ownership and evidence, attempt repair teardown and report confirmed absence or stop uncertainty. Malformed child status SHALL NOT prevent teardown through hash-bound parent routing with exact original ownership, nor permit a fabricated checkpoint/terminal state; malformed bytes SHALL remain diagnostic evidence. This command SHALL NOT authorize production execution merely by existing, nor expand prepared114/123/target, published114, ordinary, or systemd admission.
