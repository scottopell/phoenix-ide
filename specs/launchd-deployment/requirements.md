# Native macOS launchd Deployment Requirements

## Scope

Safe replacement of Phoenix's native macOS production LaunchAgent from exact checked local `HEAD`, an immutable published GitHub release, or the explicitly selected protected prepared-artifact paired upgrade under REQ-LDD-017. Linux deployment modes are outside this specification.

## Requirements

### REQ-LDD-001 — Independent activation ownership

When Phoenix initiates a production deployment, the system shall transfer activation to a launchd-owned one-shot helper before stopping the Phoenix LaunchAgent.

### REQ-LDD-002 — Complete preparation before disruption

Before stopping the running service, the system shall validate and stage the candidate binary, complete plist, rollback inputs, embedded identity, signature, and immutable activation manifest on the destination filesystem.

### REQ-LDD-003 — Single activation writer

While an activation or unresolved transaction owns the host deployment claim, the system shall reject another deployment without changing the production service or artifacts.

### REQ-LDD-004 — Immutable, secret-safe handoff

The activation helper shall be sourced from the selected immutable commit and consume only stable host-resident files and a manifest containing source identity, candidate and previous runtime identities and endpoints, artifact hashes, target paths, and rollback paths; the manifest and durable diagnostics shall not contain plist environment values.

### REQ-LDD-005 — Atomic artifact replacement

When installing a candidate or restoring a rollback artifact, the system shall fsync staged data and replace the target with an atomic rename without first unlinking the live path.

### REQ-LDD-006 — Observed launchd transitions

When changing the target job state, the system shall check launchctl exit status and poll structured job state and PID conditions to a bounded deadline.

### REQ-LDD-007 — Exact runtime verification

A deployment shall succeed only when the target job is running with a new PID and the credential-free `/api/version` endpoint reports both the expected package version and complete 40-character lowercase embedded git SHA. The candidate SHA shall equal the selected full source commit exactly; a matching prefix shall not establish candidate identity.

### REQ-LDD-008 — Verified rollback

If activation fails after disruption, the system shall atomically restore and bootstrap the previous binary and plist, verify the captured previous runtime identity at the previous service endpoint, and durably distinguish successful runtime-artifact rollback from rollback failure.

Only in this rollback role, the captured identity of an already-installed previous runtime may contain either a legacy 12-character lowercase git SHA or a full 40-character lowercase git SHA. This allowance shall not admit shortened identity for a candidate, release asset, helper, or general downgrade path and shall not establish cross-version compatibility.

Ordinary runtime-only rollback shall not restore a database or guarantee predecessor compatibility with candidate-mutated data. The explicit paired ProductConversation upgrade shall restore the verified matching database and runtime before predecessor startup under REQ-LDD-017; other rollback behavior remains governed by `specs/compatibility/requirements.md`.

### REQ-LDD-009 — Truthful durable result

After exact verification, the system shall write `deployed.sha` from the selected candidate's embedded source commit and persist a redacted terminal transaction status inspectable after the initiating connection ends.

### REQ-LDD-010 — Recoverable interruption

When status or deployment encounters a stale nonterminal transaction, the system shall expose the transaction and actionable recovery guidance without silently treating it as success.

### REQ-LDD-011 — Explicit candidate sources

The local command shall deploy exact local `HEAD` after checks and compilation and require the candidate to embed that complete 40-character lowercase commit SHA. The release command shall resolve one immutable published tag and its exact commit, select the host-architecture macOS asset, verify its `SHA256SUMS` entry, and require its complete 40-character lowercase embedded git SHA to equal that commit exactly; it shall not run repository checks, dependency installation, worktree mutation, or compilation.

The explicitly selected prepared-artifact source SHALL be admitted only under REQ-LDD-017, without local rebuild or ad-hoc resigning; it SHALL NOT expand ordinary local/release rollback guarantees.

WHEN `latest` is requested,
THE release command SHALL require the resolved tag and GitHub release metadata to identify a stable supported release.

WHEN an exact release-candidate tag is requested,
THE release command SHALL require the tag, GitHub prerelease metadata, full embedded version, and exact commit identity to agree.

### REQ-LDD-012 — Unambiguous command surface

The deployment command shall accept `prod deploy` for local `HEAD`, `prod deploy --release TAG|latest` for published releases, and the complete three-option prepared-artifact paired command under REQ-LDD-017, and shall reject positional versions with migration guidance rather than building a local source tag.

### REQ-LDD-013 — Disposable integration safety

A launchd integration harness shall use disposable labels, paths, database, and port and shall structurally refuse the production label and production resources.

### REQ-LDD-014 — Installed-state-preserving restart

When the operator requests a production restart, the system shall restart the loaded socket-activated LaunchAgent with a new PID while preserving the installed binary, plist, environment, listener, and deployed source identity byte-for-byte; it shall not build, run repository checks, read `.phoenix-ide.env`, replace installation artifacts, or unload the target job.

### REQ-LDD-015 — Independent restart ownership and fencing

Before signaling Phoenix, the system shall validate that the running process reports adoption of the launchd-owned socket and transfer restart ownership to a distinct one-shot LaunchAgent whose immutable, secret-free handoff records the expected runtime identity, previous PID, installed artifact hashes, endpoint, and target job identity. Restart and deployment shall share one mutually exclusive host-operation fence.

### REQ-LDD-016 — Exact, truthful restart result

A restart shall require the already-installed runtime to report a complete 40-character lowercase embedded git SHA and shall commit only after launchd reports a new target PID and `/api/version` reports the same exact runtime identity, with the installed artifact hashes unchanged. The rollback-only legacy identity allowance in REQ-LDD-008 shall not authorize restart of a shortened-identity installation. The system shall durably distinguish preparation failure, concurrent rejection, verified success, and failure after signaling; it shall not claim rollback when no installation artifact changed.

### REQ-LDD-017 — Supported prepared-artifact paired deployment

After helper handoff and before quiescing production, the paired helper shall physically reserve private database-plus-WAL snapshot and atomic-restore capacity. Initial reservation failure shall be reported before disruption. After quiesce required size shall be checked against the held reservation: a growth shortfall shall resume only a verified unchanged predecessor under the offline legacy/exclusivity and captured binary/configuration proofs below; otherwise the service shall remain stopped with its recovery claim retained.

Failed paired claims shall not advise marker removal based solely on helper absence. Unresolved paired recovery shall remove the target plist from the auto-loaded LaunchAgents directory into its private transaction quarantine and fsync both directories, preventing later login from automatically starting an unverified runtime. Candidate and predecessor verification shall bootstrap private transaction plists rather than publishing an unverified auto-loaded plist. The candidate plist shall be published only after exact identity verification and durable commit; the predecessor plist shall be published only after matched database authorization and predecessor verification. Publication or reservation-cleanup failure after durable commit shall remain a committed diagnostic and shall not trigger rollback or imply reboot persistence; a pending publication/cleanup diagnostic shall be persisted with the commit and its owned claim retained until both attempts have durably completed; quarantine failure shall be explicit. Reservation creation shall fsync its parent directory. Explicit retained-helper recovery shall verify offline ownership, snapshot/context, matching restoration and predecessor identity before releasing the claim; failed verification shall attempt teardown, preserve the claim, and report unconfirmed teardown.

WHEN an operator supplies `prod deploy --prepared-artifact DIR --expected-full-commit SHA --paired-database-upgrade`
THE SYSTEM SHALL require macOS launchd, require all three options together, reject `--release`, first install, and any database path change, and preserve the installed plist's environment and PATH without changing credential or model defaults.

THE SYSTEM SHALL accept only a protected `prepare-main` receipt for the host architecture whose exact full commit, version, Developer ID signature, hardened runtime, accepted notarization, stapled ticket, Gatekeeper result, embedded-helper equivalence, and standalone SHA-256 bytes all verify. The candidate source kind SHALL be `prepared_artifact`, distinct from the clean controller HEAD source commit. The system SHALL never ad-hoc resign the prepared binary.

THE handoff manifest SHALL structurally record the paired ProductConversation database-upgrade mode, exact controller source commit, helper bytes, captured predecessor binary/plist identities, database path, fixed private backup/proof destinations, and transaction identity. The snapshot proof and evolving rollback/status observations SHALL be separate durable records bound to that immutable manifest; they SHALL NOT be fabricated or added by mutating the handoff. The helper SHALL reject missing or inconsistent fields and SHALL require the controller helper bytes to match the recorded clean controller source and protocol.

After backend-managed quiesce confirms the predecessor stopped, THE helper SHALL prove no other process has the database, WAL, or SHM open using bounded macOS `lsof`, take a SQLite backup-API snapshot into a private mode-700 transaction directory with a mode-600 database, validate integrity, and durably verify the snapshot before candidate startup. It SHALL never raw-copy a live database.

IF candidate activation or health verification fails, THE helper SHALL stop the candidate first, re-prove exclusive offline ownership, restore and integrity-check the matching snapshot while removing stale WAL/SHM only under that proof, atomically restore the predecessor binary/plist, and start the predecessor only after database restoration. If any proof fails it SHALL leave the service stopped, persist actionable recovery status, and retain the active claim. Existing runtime-only rollback remains unchanged. Before a snapshot proof exists, a failure MAY resume the predecessor only after confirmed teardown, exclusive offline ownership, legacy ledger/table eligibility, and captured binary/configuration checksum equality prove that candidate mutation has not occurred. This path SHALL NOT claim database restoration. An interrupted pre-handoff preparation, whether or not its immutable manifest has already been persisted, SHALL release only its matching claim after durable terminal status, a dead recorded preparation PID, and confirmed target helper absence; live, reused, missing, or unproven process identity SHALL retain ownership. Successful commit SHALL remove only the temporary database-adjacent restore reservation and fsync its parent, retaining the audit snapshot/proof.
