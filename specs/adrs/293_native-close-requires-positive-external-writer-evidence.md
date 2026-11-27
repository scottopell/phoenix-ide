# ADR-293: Native physical Close requires positive external writer evidence to block on a process

- **Status:** Accepted
- **Date:** 2026-11-27
- **Affects:** REQ-WL-002e; admitted native Worktree physical retirement

## Context

Ordinary shared macOS and Linux hosts can deny inspection of other processes even at the same effective UID. A process that cannot be inspected is not evidence that the exact worktree has a writer, but it is also not evidence that it has none. The previous rule blocked physical removal in either case.

## Options considered

1. Keep all process inspection gaps blocking. This prevents more potential races but prevents clean retirement on some ordinary shared hosts.
2. Move the checkout somewhere private without deleting it. This can clear a pathname but does not reclaim disk space.
3. Require positive external writer evidence to block the admitted native physical step. Keep process gaps as unknown, and keep all target safety checks. Selected with explicit acceptance of the remaining race.

## Decision

After confirmed shutdown of Phoenix-owned resources, an inspection gap for external processes alone does not prevent authorized native physical worktree retirement. Continue examining independent evidence rather than letting a gap mask a verifiable writer. A candidate whose process or descriptor identity cannot be verified is not positive evidence. Retain a current positive observation if found, otherwise a representative typed gap, under the existing exact-run immutable evidence contract.

This partially supersedes the external process-gap blocking policy in ADR-241 and ADR-286 for this boundary only. Positive target-bound authority, failed evidence persistence, target inspection and loss checks, exact resource and registration identity, custody, deadlines, and truthful incomplete outcomes remain in force. Pre-admission retry and the strict legacy paths do not gain an exception. This does not authorize changes to retained failed environments.

## Consequences

An undetected process can still race deletion and lose a concurrent save or Git change. Success proves logical removal and Git registration absence, not immediate space reclamation when handles remain open. Hosted regressions and installed acceptance with fresh disposable environments on ordinary shared hosts are separate gates; this record is not evidence that either passed.

## References

- [Work Lifecycle requirements](../work-lifecycle/requirements.md), REQ-WL-002e.
- [ADR-241](241_close-writer-authority-is-immutable-run-evidence.md): immutable exact-run evidence.
- [ADR-286](286_close-uses-exact-blockers-and-explicit-retry-only.md): exact blockers and explicit retry.
