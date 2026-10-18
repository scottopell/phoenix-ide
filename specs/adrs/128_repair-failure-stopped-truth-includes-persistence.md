# ADR-128: Repair failure stopped truth includes persistent auto-start absence

- **Status:** Accepted
- **Date:** 2026-10-18
- **Affects:** REQ-LDD-025, exact current-schema launchd repair failure settlement

## Context

The exact current-schema repair publishes the repair LaunchAgent plist before removing the old restore reservation, persisting terminal child status, and releasing the original claim. A failure after publication can confirm that the current launchd job is absent while leaving the persistent plist available for a later login or reboot to load. Reporting `failed_stopped` from process absence alone would therefore assert a stable stopped condition that the host has not established.

The target plist path can also contain unknown or mismatched bytes. Deleting those bytes without proving that they are the sealed repair plist would turn failure settlement into unowned cleanup and destroy diagnostic evidence.

## Options considered

1. Treat process absence as sufficient stopped truth. Rejected because persistent launchd registration can reactivate the process after login or reboot.
2. Unlink the target path after hashing it in place. Rejected because a pathname replacement between hash and unlink can destroy unknown evidence.
3. Use bounded atomic quarantine passes, fsync target-path absence, and defer bounded no-follow evidence validation to resume admission. Chosen because stopped truth no longer depends on interpreting untrusted bytes, resume authority remains bound to the sealed child, and a same-user writer cannot hold the controller lock indefinitely by continually recreating the target.

## Decision

Treat process absence and persistent auto-start absence as separate facts. Repair failure settlement reports `failed_stopped` only after both facts are established.

Failure settlement uses a bounded sequence of atomic renames to move each target entry observed during settlement into transaction-owned quarantine without reading or deleting it, fsyncs the containing directory, stops and confirms process absence, then repeats the bounded quarantine/fsync step to close changes made during teardown and reconfirms process absence after that final persistence pass. The final probe is authoritative even if an earlier stop or absence probe was ambiguous. It reports `failed_stopped` when the auto-loaded target path is durably absent, regardless of quarantined content; inability to isolate, fsync or confirm target absence, including target recreation through the attempt bound, or process reappearance selects `stop_unconfirmed`. Before failure teardown, the controller independently requires the fixed Phoenix service label and its canonical owning-user LaunchAgents target, preventing mutable child or parent routing from redirecting quarantine or stop. After the sealed child has been authenticated, resume bounds quarantine count and aggregate bytes while reserving entry and byte headroom for both publication retirements and both bounded settlement passes; a failure-state retry may consume that reserved capacity and validates quarantines through size-bounded no-follow nonblocking descriptors, requires stable file identity, type, size, and change metadata for the complete read, verifies that each pathname remains bound to the read descriptor, retains and revalidates each accepted file's metadata token at admission close, and rejects changes across repeated complete quarantine enumerations or the containing directory's change token. Exact repair evidence is admissible, while unknown, mutable, replaced, newly added, oversized, nonregular, or unreadable evidence refuses resume and remains untouched. Publication uses a no-replace commit and rechecks the target after directory fsync, so bytes appearing after admission are never overwritten. Its temporary is atomically retired into retained transaction-owned quarantine and authenticated there rather than pathname-deleted, so a replacement is preserved and blocks success.

Both failure states retain the exact original claim, sealed child evidence, parent evidence, and last durable resume checkpoint. This decision does not alter terminal successful repair behavior or the completed production transaction that exposed the review question.

## Consequences

- Login and reboot cannot silently reactivate an exact repair reported as `failed_stopped`.
- Target isolation, repeated recreation through the bounded attempts, or directory-fsync ambiguity remains explicitly non-terminal and recoverable as `stop_unconfirmed`.
- A transient first absence-probe failure does not force stop uncertainty when the authoritative post-persistence probe confirms absence.
- Initial admission refuses before consuming quarantine capacity needed by both publication retirements and both bounded settlement passes; a failure-state retry can consume the escrowed capacity instead of being stranded by the immediately preceding failure.
- Failure settlement never parses or deletes quarantined evidence.
- Publication retirement preserves exact evidence on success and preserves mismatched replacement evidence on failure instead of deleting either by pathname.
- Resume continues to use the sealed child and republishes its exact plist only after bounded validation of retained target evidence.

## References

- `specs/launchd-deployment/requirements.md` — REQ-LDD-025
- `specs/launchd-deployment/launchd-deployment.allium`
- [ADR-126](126_seal-exact-current-schema-forward-replacement.md)
