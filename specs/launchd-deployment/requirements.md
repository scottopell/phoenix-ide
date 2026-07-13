# Native macOS launchd Deployment Requirements

## Scope

Safe replacement of Phoenix's native macOS production LaunchAgent from a local checkout or a published GitHub release. Linux deployment modes are outside this specification.

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

A deployment shall succeed only when the target job is running with a new PID and the credential-free `/api/version` endpoint reports both the expected package version and embedded git SHA.

### REQ-LDD-008 — Verified rollback

If activation fails after disruption, the system shall atomically restore and bootstrap the previous binary and plist, verify the previous exact runtime identity at the previous service endpoint, and durably distinguish successful rollback from rollback failure.

### REQ-LDD-009 — Truthful durable result

After exact verification, the system shall write `deployed.sha` from the selected candidate's embedded source commit and persist a redacted terminal transaction status inspectable after the initiating connection ends.

### REQ-LDD-010 — Recoverable interruption

When status or deployment encounters a stale nonterminal transaction, the system shall expose the transaction and actionable recovery guidance without silently treating it as success.

### REQ-LDD-011 — Explicit candidate sources

The local command shall deploy exact local `HEAD` after checks and compilation. The release command shall resolve one immutable published tag and its exact commit, select the host-architecture macOS asset, verify its `SHA256SUMS` entry and require its embedded git SHA to match that commit, and shall not run repository checks, dependency installation, worktree mutation, or compilation.

### REQ-LDD-012 — Unambiguous command surface

The deployment command shall accept `prod deploy` for local `HEAD` and `prod deploy --release TAG|latest` for published releases, and shall reject positional versions with migration guidance rather than building a local source tag.

### REQ-LDD-013 — Disposable integration safety

A launchd integration harness shall use disposable labels, paths, database, and port and shall structurally refuse the production label and production resources.
