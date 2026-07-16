# Cross-Platform Production Deployment Requirements

## Scope

Safe replacement and operation of native Phoenix production runtimes on macOS launchd, Linux systemd, and Linux hosts without systemd. The deployment source is either exact checked local `HEAD` or an immutable published release. This specification owns shared cross-platform guarantees and the release assets required to satisfy them. `specs/launchd-deployment/requirements.md` cumulatively refines launchd-specific mechanics; it does not redefine the shared guarantees.

## Requirements

### REQ-PD-001 — Deterministic backend selection

When an operator invokes a production command, the system shall select launchd on macOS, systemd when Linux is running systemd, and the bare-Linux supervisor on Linux without systemd, and shall report the selected backend in status output.

### REQ-PD-002 — Explicit candidate sources

The local deployment command shall run required checks, build exact local `HEAD`, require the embedded identity to match that commit, and stage the resulting binary. The release deployment command shall resolve `latest` at most once or use the requested tag, bind the tag to one immutable commit, select the host target asset, verify the published checksum and embedded identity, and shall not run repository checks, install dependencies, mutate the worktree, or compile.

### REQ-PD-003 — Complete preparation before disruption

Before disrupting a running runtime, the system shall stage and validate the candidate, backend configuration, exact `.phoenix-ide.env` snapshot, rollback inputs, runtime identities and endpoints, destination-space reservations, artifact hashes, backend activation program, immutable handoff, and initial durable transaction status.

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

A deployment shall commit only after observing a new backend-owned runtime process and verifying that its credential-free version endpoint reports the exact expected package version and full embedded git SHA. Bare Linux shall additionally bind verification to the supervisor's direct child PID and `/proc` start time.

### REQ-PD-010 — Verified rollback

If activation fails after disruption, the activation owner shall stop the candidate, atomically restore the previous binary, backend configuration, environment snapshot, and service state, verify the previous exact identity at its previous endpoint, restore the previous deployed SHA, and durably distinguish successful rollback from rollback failure.

### REQ-PD-011 — Truthful durable status and recovery

After exact verification, the system shall write `deployed.sha` from the selected candidate's embedded source commit and durably persist `committed` before releasing the claim. A nonterminal or interrupted transaction shall remain visible and actionable and shall never be inferred as successful solely from a PID, active unit, responsive port, or installed file.

### REQ-PD-012 — Configuration source of truth

Modern deployment shall load `.phoenix-ide.env` once and use that exact snapshot for preflight, installation, and candidate endpoint selection. Runtime status shall inspect installed configuration, rollback shall use the previous installed snapshot, and modern operation shall ignore rather than migrate or consult a legacy launchd override store, systemd drop-in, or inferred detached-daemon environment.

### REQ-PD-013 — Consistent operator surface

Each backend shall support `prod deploy`, `prod deploy --release TAG|latest`, `prod status`, and `prod stop`. Positional deployment versions shall be rejected with migration guidance. `prod set` and `prod unset` shall reject configuration mutation and direct the operator to edit `.phoenix-ide.env`; they shall not create backend-specific override state.

### REQ-PD-014 — Persistent bare-Linux ownership

On bare Linux, an owner-only supervisor shall directly parent Phoenix, authenticate clients through an owner-only Unix socket and Linux peer credentials, accept only a transaction ID plus manifest hash, and reject stale, replayed, concurrent, malformed, or tampered handoffs. `prod stop` shall stop the managed child without stopping the supervisor.

### REQ-PD-015 — Honest bare-Linux reboot persistence

Bare-Linux installation shall start the supervisor independently for the active boot and install an owner `@reboot` entry when a compatible crontab exists. Otherwise it shall report exact host rc guidance and shall not claim reboot persistence.

### REQ-PD-016 — Disposable integration safety

Integration harnesses shall use randomized virtual machines or containers, units, labels, paths, ports, databases, and transaction identities; shall structurally refuse production resources; and shall not mount production data, host credentials, or the host worktree into authoritative activation tests.

### REQ-PD-017 — Supported published Linux assets

Published releases shall provide checksummed `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` binaries in addition to supported macOS assets, and release candidate selection shall reject an asset whose target, checksum, version, or embedded commit does not match the selected release.
