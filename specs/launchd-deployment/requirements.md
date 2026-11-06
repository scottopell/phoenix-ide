# Native macOS launchd Deployment Requirements

## Scope

Ordinary local-HEAD and published-release replacement of the native macOS LaunchAgent, with candidate-owned migration and operator recovery after disruption. Linux behavior is specified separately in `specs/production-deployment/requirements.md`.

## Requirements

### REQ-LDD-001 — Independent activation ownership

When Phoenix initiates production deployment, the system shall transfer activation to a distinct launchd-owned one-shot helper before disrupting the target LaunchAgent.

### REQ-LDD-002 — Complete preparation before disruption

Before disrupting the service, the system shall validate the exact candidate identity and source, binary hash, complete plist and string-valued environment, protocol-2 handoff, and destination-filesystem atomic installation inputs. Preparation shall not inspect the production database or run migrations.

### REQ-LDD-003 — Single activation writer

While any deploy or restart marker exists, the system shall reject another operation. Marker contents or terminal status shall not authorize automatic reclamation. The deployment helper shall release only its own claim after successful commit or pre-disruption preparation failure. Restart shall retain its separate terminal-result release behavior.

### REQ-LDD-004 — Immutable, secret-safe handoff

The helper shall consume immutable host-resident inputs for an ordinary `local_head` or `published_release` candidate. The manifest shall bind source, complete candidate identity, binary/plist hashes, target paths, helper job identity and credential-free endpoint without copying plist environment values into status or the manifest. The installer shall verify helper protocol 2 before disruption; the helper shall reject older manifests without conversion or replay.

### REQ-LDD-005 — Atomic artifact replacement

When installing the candidate, the system shall fsync staged data and atomically replace the binary without unlinking its live path first. It shall move the canonical installed plist to the private transaction before stopping the predecessor, bootstrap the candidate with its private plist, and publish that same inode by hard link and atomic rename only after runtime verification.

### REQ-LDD-006 — Observed launchd transitions

When stopping or starting the target, the helper shall check launchctl results and poll structured state and PID conditions to bounded deadlines. A teardown attempt shall not be reported as confirmed absence when observation fails.

### REQ-LDD-007 — Exact runtime verification

A deployment shall succeed only when the target has a new PID, the credential-free `/api/version` endpoint reports the exact expected package version and complete 40-character lowercase source SHA, and the target retains that PID through verification. Prefix equality shall not establish identity.

### REQ-LDD-008 — Withdrawn deployment recovery contract

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-229.

### REQ-LDD-009 — Truthful durable result

After exact verification and plist publication, the system shall write `deployed.sha` from the selected source, persist `committed`, and release its owned claim. Status shall remain inspectable after the initiating connection ends and shall expose the active owner even when shared status is missing or belongs to another transaction.

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

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-229.

### REQ-LDD-018 — Withdrawn deployment recovery contract

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-229.

### REQ-LDD-019 — Withdrawn deployment recovery contract

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-229.

### REQ-LDD-020 — Withdrawn deployment recovery contract

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-229.

### REQ-LDD-021 — Withdrawn deployment recovery contract

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-229.

### REQ-LDD-022 — Withdrawn deployment recovery contract

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-229.

### REQ-LDD-023 — Withdrawn deployment recovery contract

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-229.

### REQ-LDD-024 — Withdrawn deployment recovery contract

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-229.

### REQ-LDD-025 — Withdrawn deployment recovery contract

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-229.

### REQ-LDD-026 — Withdrawn deployment recovery contract

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-229.

### REQ-LDD-027 — Withdrawn deployment recovery contract

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-229.

### REQ-LDD-028 — Withdrawn deployment recovery contract

Withdrawn; this ID is reserved and grants no deployment or recovery guarantee. See ADR-229.

### REQ-LDD-029 — Candidate-owned migration

After confirmed predecessor stop and candidate installation, the helper shall durably record `migration_started` before invoking the exact candidate with `--migrate-only` once, with the staged environment, inherited diagnostic output and a bounded 600-second timeout. The candidate owns database eligibility, migration and reconciliation. Routine deployment shall not scan the database, parse migration registries, snapshot, rehearse or implement schema-specific upgrade admission.

### REQ-LDD-030 — Manual recovery after disruption

If any step fails after disruption begins, the helper shall attempt target stop, record `failed` with one human-readable failure containing the failed phase, error and any stop uncertainty, and retain its claim and transaction artifacts. It shall not start the predecessor, restore a database, or resume/replay the transaction. In-app deployment approval shall remain blocked pending operator recovery.

### REQ-LDD-031 — Explicit operator recovery boundary

Recovery shall require operator inspection of logs, retained transaction evidence and actual service/helper state. Only after confirming helper absence and resolving the actual runtime state may the operator explicitly clear the ownership marker and deploy a chosen forward candidate. Starting an older binary requires a matching database restore while all database users are stopped. Historical transaction files shall remain diagnostic evidence, without a conversion or retained-helper recovery API.

### REQ-LDD-032 — Best-effort committed cleanup

After successful commit and claim release, the helper may remove only its temporary candidate and previous binary copies. Cleanup failure shall report a warning without stopping the verified runtime or changing the committed result.
