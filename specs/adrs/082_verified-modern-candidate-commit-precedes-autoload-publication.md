# ADR-082: Verified modern candidate commit precedes autoload publication

- **Status:** Accepted
- **Date:** 2026-10-05
- **Affects:** launchd deployment, production deployment, compatibility

## Context

ADR-080 established explicit ordinary modern migration failure as stopped/fenced with manual matched recovery. Publishing the candidate's autoload plist before recording durable commit leaves a power-loss window: an unresolved activating receipt can coexist with a candidate that starts after login/reboot and accepts writes. Treating that candidate as failed and restoring its predecessor backup would discard those writes. ADR-077 already establishes publication-only committed finalization for the separate paired path; this decision applies the same write-ownership boundary to explicit ordinary modern migration without widening legacy eligibility.

## Options considered

1. Publish autoload configuration while still activating and stop/restore on interruption: rejected; autoload can precede the failure receipt and accepted writes are not rollback data.
2. Never publish ordinary migration candidates: rejected; successful deployment would lack verified login/reboot persistence.
3. Durably commit the exact verified private candidate before enabling autoload, then finalize publication only: selected. Publication failures are committed diagnostics, not migration rollback authorization.

## Decision

After exact candidate runtime/private configuration verification and deployed identity, persist `committed` with `finalization_pending` before autoload plist publication. Until that checkpoint, failure remains stopped/fenced and manual matched predecessor recovery is supported. After that checkpoint, candidate database state and accepted writes are authoritative: publication interruption retains commit/claim and never authorizes predecessor bootstrap, database inspection/replay or restoring the old backup.

The existing `prod resume-migration TXN` dispatches committed ordinary transactions to publication-only finalization: retained helper/absence, operation lock, claim, exact running candidate/binary/private loaded configuration/deployed identity, and absent-or-identical published inode. It publishes only the captured candidate plist, fsyncs, settles status and durably releases only owned claim. If the candidate is stopped or unknown, it refuses without starting anything; this is not a generic reboot recovery framework. Activation acceptance still requires pending finalization to be cleared and claim removal proven.

Resume validation failures before predecessor startup preserve the original candidate failure and append separately labelled resume-attempt diagnostics, so later success does not replace the activation audit reason with a restore-check error.

## Consequences

The pre-commit fail-stopped policy remains bounded, but publication errors after a verified candidate commit preserve the running candidate and its writes rather than manufacturing a failed migration. An interrupted prepublication commit can lack login/reboot persistence and requires explicit verified publication finalization. Crash regressions cover before commit, after checkpoint, status-sync interruption, after publication and final status, plus publication errors; they preserve later writes and reject stopped-candidate replay. No host activation, downgrade guarantee, or automatic database restoration is introduced.
