# Native macOS launchd Deployment Requirements

## Scope

Ordinary local-HEAD and published-release replacement of the native macOS LaunchAgent, with candidate-owned migration and operator recovery after disruption. Linux behavior is specified separately in `specs/production-deployment/requirements.md`.

## Requirements

### REQ-LDD-001 — Independent activation ownership

When Phoenix initiates production deployment, the system shall transfer activation to a distinct launchd-owned one-shot helper before disrupting the target LaunchAgent.

### REQ-LDD-002 — Complete preparation before disruption

Before disrupting the service, the system shall validate the exact candidate identity and source, binary hash, complete plist and string-valued environment, protocol-3 handoff, and destination-filesystem atomic installation inputs. For an existing database, the independent helper shall check space for a native SQLite backup plus bounded operational overhead before quarantining the plist and stopping the service. For a proven first install without a database or managed predecessor, it shall instead check bounded first-install capacity. Neither check guarantees migration peak space. Candidate migration shall not run during preparation.

### REQ-LDD-003 — Single activation writer

While any deploy or restart marker exists, the system shall reject another operation. Marker contents or terminal status shall not authorize automatic reclamation. The deployment helper shall release only its own claim after successful commit or pre-disruption preparation failure. Restart shall retain its separate terminal-result release behavior.

### REQ-LDD-004 — Immutable, secret-safe handoff

The helper shall consume immutable host-resident inputs for an ordinary `local_head` or `published_release` candidate. The manifest shall bind source, complete candidate identity, binary/plist hashes, target paths, helper job identity and credential-free endpoint without copying plist environment values into status or the manifest. The installer shall verify deploy-helper protocol 3 before disruption; the helper shall reject older deploy manifests without conversion or replay. Installed-runtime restart retains its separate protocol 2.

### REQ-LDD-005 — Atomic artifact replacement

When installing the candidate, the system shall fsync staged data and atomically replace the binary without unlinking its live path first. It shall move the canonical installed plist to the private transaction before stopping the predecessor, bootstrap the candidate with its private plist, and publish that same inode by hard link and atomic rename only after runtime verification.

### REQ-LDD-006 — Observed launchd transitions

When stopping or starting the target, the helper shall check launchctl results and poll structured state and PID conditions to bounded deadlines. A teardown attempt shall not be reported as confirmed absence when observation fails.

### REQ-LDD-007 — Exact runtime verification

A deployment shall succeed only when the target has a new PID, the credential-free `/api/version` endpoint reports the exact expected package version and complete 40-character lowercase source SHA, and the target retains that PID through verification. Prefix equality shall not establish identity.

### REQ-LDD-008 — Withdrawn deployment recovery contract

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-230.

### REQ-LDD-009 — Truthful durable result

After exact verification and plist publication, the system shall write `deployed.sha` from the selected source, persist `committed`, and attempt to release its owned claim. Durable `committed` shall be terminal for runtime behavior: claim-release, reporting or cleanup errors shall not stop the verified runtime or change its committed result. Any surviving claim shall remain visible for operator inspection. Status shall remain inspectable after the initiating connection ends and shall expose the active owner even when shared status is missing or belongs to another transaction. Backup verification, first-install absence and recovery claims shall derive only from transaction-matched durable proof; missing proof shall report the boundary and backup as unknown, not infer success. Proven first-install absence shall record private `database_source: absent` and `backup: null`, not a verified-backup claim.

### REQ-LDD-010 — Interrupted ownership remains visible

When an unfinished deployment is interrupted or fails with a surviving ownership claim, the system shall leave that claim and its diagnostic artifacts for operator inspection. Interruption after completed commit and claim release shall not recreate ownership. Status shall not infer success or release ownership from an absent helper, responsive endpoint, installed binary or terminal status alone.

### REQ-LDD-011 — Explicit ordinary candidate sources

The local command shall check and build exact local `HEAD` and require that full embedded source identity. The release command shall select one immutable published tag and commit, verify the host asset checksum and full embedded version/SHA, and install without repository checks or compilation. `latest` shall identify a stable supported release; an exact release-candidate tag shall agree with prerelease metadata and full embedded identity. Prepared-artifact macOS sources shall be rejected.

### REQ-LDD-012 — Unambiguous command surface

The macOS command shall accept `prod deploy` and `prod deploy --release TAG|latest`, and shall reject positional versions with migration guidance. It shall expose no prepared, paired, current-schema, refresh, repair, resume or finalization deployment route.

### REQ-LDD-013 — Disposable integration safety

A launchd integration harness shall use disposable labels, paths, database and port and shall structurally refuse production resources.

### REQ-LDD-014 — Installed-state-preserving restart

When the operator requests a production restart, the system shall restart the loaded socket-activated LaunchAgent with a new PID while preserving the installed binary, plist, environment, listener, and deployed source identity byte-for-byte; it shall not build, run repository checks, read `.phoenix-ide.env`, replace installation artifacts, or unload the target job.

### REQ-LDD-015 — Independent restart ownership and fencing

Before signaling Phoenix, the system shall validate that the running process reports adoption of the launchd-owned socket and transfer restart ownership to a distinct one-shot LaunchAgent whose immutable, secret-free handoff records the expected runtime identity, previous PID, installed artifact hashes, endpoint, and target job identity. Restart and deployment shall share one mutually exclusive host-operation fence.

### REQ-LDD-016 — Exact, truthful restart result

A restart shall require the installed runtime to report a complete 40-character lowercase embedded SHA, and shall commit only after observing a new target PID with the same exact runtime identity and unchanged installed artifact hashes. The system shall distinguish preparation failure, concurrency rejection, verified success and failure after signaling; it shall not claim rollback when artifacts were not replaced.

### REQ-LDD-017 — Withdrawn deployment recovery contract

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-230.

### REQ-LDD-018 — Withdrawn deployment recovery contract

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-230.

### REQ-LDD-019 — Withdrawn deployment recovery contract

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-230.

### REQ-LDD-020 — Withdrawn deployment recovery contract

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-230.

### REQ-LDD-021 — Withdrawn deployment recovery contract

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-230.

### REQ-LDD-022 — Withdrawn deployment recovery contract

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-230.

### REQ-LDD-023 — Withdrawn deployment recovery contract

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-230.

### REQ-LDD-024 — Withdrawn deployment recovery contract

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-230.

### REQ-LDD-025 — Withdrawn deployment recovery contract

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-230.

### REQ-LDD-026 — Withdrawn deployment recovery contract

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-230.

### REQ-LDD-027 — Withdrawn deployment recovery contract

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-230.

### REQ-LDD-028 — Withdrawn deployment recovery contract

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-230.

### REQ-LDD-029 — Candidate-owned migration

Filesystem paths supplied to SQLite URI connections shall be URI-encoded without changing the addressed file. Capacity inspection, writer reservation and backup shall address the same configured database; missing sources shall not be created.

For an existing database, after confirmed predecessor stop the independent helper shall create a native SQLite backup, check its integrity and compare source and backup metadata and stored rows in lockstep, then durably retain the verified backup and record its path, size and SHA-256 as transaction proof. Logical equivalence shall preserve SQLite storage classes and row identity (rowid for rowid tables and primary-key identity for `WITHOUT ROWID` tables), and shall compare persisted FTS5 shadow-table rows rather than virtual-table query results. Unsupported virtual-table modules and tables shadowing all rowid aliases shall fail closed. Backup creation, integrity checking, logical comparison, fsync and proof hashing shall share one 900-second backup-verification deadline; timeout or mismatch shall fail deployment before candidate installation or migration. A backup failure shall prevent candidate installation and migration. Only when the database path is absent and there is no managed predecessor, installed binary, installed plist or loaded service may the helper use the first-install path instead: it shall record transaction-matched private proof with `database_source: absent` and `backup: null`, without manufacturing a backup. It shall recheck that the database path is still absent before the durable candidate boundary; an unexpectedly present database shall fail before candidate installation or migration. The helper shall recheck a small operational space reserve, then record the candidate boundary as attempted before replacing the binary or invoking migration; an attempted or uncertain boundary shall never authorize predecessor fallback. It shall durably record `migration_started` before invoking the exact candidate with `--migrate-only` once, with the staged environment, inherited diagnostic output and a bounded 600-second timeout. The candidate owns database eligibility, migration and reconciliation. The helper shall not parse migration registries, rehearse upgrades or implement schema-specific upgrade admission. The operational reserve does not promise space for migration peak usage. The update requires maintenance exclusivity: no external database clients may write during the update, including after handoff to the candidate. For an existing database, after stopping the managed service the helper shall acquire a bounded SQLite `BEGIN IMMEDIATE` writer reservation and hold it across backup verification and durable candidate boundary recording, releasing it before candidate migration or predecessor recovery. This reservation does not police external clients after handoff to the candidate.

Backup verification shall emit a bounded, content-free diagnostic summary on success or failure: current stage, cumulative stage durations, stage-entry budget, ordinal table identity and compared row/cell progress. Diagnostics shall not include database names, schema SQL, stored values or raw SQLite error details, alter verification predicates, reset the shared deadline or mask an operation failure. Cells count attempted value comparisons; rows count completely matched rows. Diagnostic output failure shall not determine deployment success.

### REQ-LDD-030 — Manual recovery after disruption

If a step fails after disruption but before the candidate boundary, the helper may start only a captured, unchanged predecessor after positively observing its stop and PID absence, verifying binary/plist identity and target absence, then durably publishing the unchanged canonical plist and bootstrapping that path before checking the recovered runtime's exact identity and new stable PID. The loaded plist shall remain the same file required by normal restart preflight. It shall report recovery as verified only after these checks; a failed or uncertain recovery shall retain the quarantined plist and claim for operator action, without claiming a stopped target. If the boundary was attempted or its status is uncertain, any failure before durable `committed` shall attempt target stop, record `failed` with phase, error and stop uncertainty, and retain claim, transaction evidence and the verified backup if an existing database was backed up; a proven first install retains absence proof instead. It shall not start the predecessor, automatically restore a database, or resume/replay the transaction. In-app deployment approval shall remain blocked pending operator recovery where ownership remains.

### REQ-LDD-031 — Explicit operator recovery boundary

Unresolved recovery shall require operator inspection of logs, transaction-matched proof (including the retained verified backup for an existing database or first-install absence proof) and actual service/helper state. Only after confirming helper absence and resolving the actual runtime state may the operator explicitly clear the ownership marker and deploy a chosen forward candidate. A preboundary verified recovery of the unchanged predecessor does not require a database restore; after an attempted or uncertain candidate boundary, starting an older binary requires a matching database restore offline while all database users are stopped. A retained backup is recovery evidence, not an automatic restore or a claim that postboundary writes are reversible. Historical transaction files shall remain diagnostic evidence, without a conversion or retained-helper recovery API.

### REQ-LDD-032 — Best-effort committed bookkeeping

After durable commit, claim release and reporting shall be best effort. If claim release does not complete, the helper shall retain its temporary binary copies and attempt a warning directing operator inspection of ownership. It shall not recreate an already removed claim. After completed claim release, the helper may remove only its temporary candidate and previous binary copies. Cleanup failure shall attempt a warning. Warning or success-report output failure shall not stop the verified runtime, change committed status, or cause a new activation failure.
