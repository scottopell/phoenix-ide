# ADR-128: Repair failure stopped truth includes persistent auto-start absence

- Status: Accepted
- Date: 2026-10-18
- Affects: REQ-LDD-025, exact current-schema launchd repair failure settlement

## Context

The exact current-schema repair publishes the repair LaunchAgent plist before removing the old restore reservation, persisting terminal child status, and releasing the original claim. A failure after publication can confirm that the current launchd job is absent while leaving the persistent plist available for a later login or reboot to load. Reporting `failed_stopped` from process absence alone would therefore assert a stable stopped condition that the host has not established.

The target plist path can also contain unknown or mismatched bytes. Deleting those bytes without proving that they are the sealed repair plist would turn failure settlement into unowned cleanup and destroy diagnostic evidence.

## Decision

Treat process absence and persistent auto-start absence as separate facts. Repair failure settlement reports `failed_stopped` only after both facts are established.

When the target plist appears absent, failure settlement fsyncs its containing directory and confirms absence again before treating persistent auto-start absence as durable. When a target exists, settlement atomically renames that directory entry into a transaction-owned quarantine before hashing it. Exact sealed repair bytes are removed and fsynced; unknown bytes remain quarantined as evidence and select `stop_unconfirmed`. Lookup errors, symlinks, unreadable state, and removal or fsync ambiguity also select `stop_unconfirmed`. Resume accepts only confirmed absence or the exact sealed repair plist and refuses while quarantine evidence remains. Publication uses a no-replace commit and rechecks the target after directory fsync, so bytes appearing after admission are never overwritten either.

Both failure states retain the exact original claim, sealed child evidence, parent evidence, and last durable resume checkpoint. This decision does not alter terminal successful repair behavior or the completed production transaction that exposed the review question.

## Consequences

- Login and reboot cannot silently reactivate an exact repair reported as `failed_stopped`.
- A target-plist unlink or directory-fsync ambiguity remains explicitly non-terminal and recoverable as `stop_unconfirmed`.
- Failure settlement removes only bytes already owned by the sealed repair identity.
- Resume continues to use the sealed child and republishes its exact plist only after full retained admission.

## References

- `specs/launchd-deployment/requirements.md` — REQ-LDD-025
- `specs/launchd-deployment/launchd-deployment.allium`
- [ADR-126](126_seal-exact-current-schema-forward-replacement.md)
