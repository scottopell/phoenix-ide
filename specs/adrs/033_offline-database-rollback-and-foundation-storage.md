# ADR-033: Database rollback is offline and Foundation observations use relational scalar storage

- **Status:** Accepted
- **Date:** 2026-08-14
- **Affects:** REQ-GITREP-001, REQ-GITREP-002, REQ-GITREP-003, REQ-GITREP-004; `RepositoryLocatorObservation`, `DefaultBranchObservation`

## Context

The GitRepository Foundation migration needs a rollback contract, a database-replacement contract, and a durable representation for observation time. A broader design attempted to support an old Phoenix binary opening a database migrated by a newer binary, detect a live database file replaced at the same path, persist a logical database-instance identity, and validate RFC 3339 text inside SQLite. Each promise introduced evidence and fencing machinery without adding user value to the dormant Foundation.

Phoenix deployments already control the process lifecycle. Database restore is an operator action rather than an online repository-domain transition. Observation time is a scalar used for freshness and ordering, not user-authored text.

## Options considered

1. **Forward-compatible rollback and live replacement detection** — keep old binaries compatible with newer schemas, persist database-instance identity, and bind compatibility evidence across live file replacement. This offers a wider operational envelope but requires long-lived compatibility, replacement, and fencing machinery.
2. **Offline paired rollback with text timestamps** — stop Phoenix, restore the matching database backup, and restart the matching binary, while retaining canonical RFC 3339 observation text. This removes cross-version compatibility but keeps a parser-shaped relational constraint and multiple textual encodings to manage.
3. **Offline paired rollback with scalar observation time** — stop Phoenix before database restore or replacement, restore the database backup paired with the target binary, and store new observation times as non-negative INTEGER Unix microseconds.

## Decision

Phoenix uses option 3.

A rollback that changes binary versions is an offline paired operation: stop Phoenix, restore the pre-upgrade database backup, then start the matching binary. Phoenix does not promise that an older binary can open a database migrated by a newer binary. This decision adds no automatic backup or version-management subsystem.

Database restore or replacement is permitted only while Phoenix is stopped. The running process does not detect or support replacement of its SQLite file at the same path. Process-local database and exclusion-operation bindings remain sufficient for the dormant catch-up/readiness lifecycle; no persisted database-instance identity, replacement fence, generation, or compare-and-swap token is created.

New GitRepository locator and default-branch observation times are stored as explicitly named INTEGER Unix-microsecond columns. Their schema requires SQLite INTEGER storage and a non-negative value. Rust converts clock observations to Unix microseconds at the persistence boundary. Locator paths and branch names reject embedded NUL because they are OS/Git values rather than opaque identity bytes.

## Consequences

- **Positive:** rollback and restore semantics match the actual process lifecycle and avoid cross-version database compatibility scaffolding.
- **Positive:** impossible live-replacement states are excluded by an operational precondition rather than detected with dormant fencing machinery.
- **Positive:** observation times have one compact, order-preserving relational representation with straightforward storage-class and range constraints.
- **Negative:** operators must retain and restore the database backup that matches the binary version being restored.
- **Negative:** Phoenix must be stopped during database replacement or restore; live replacement is unsupported.
- **Negative:** observation timestamps before the Unix epoch are not representable in these Foundation tables.
- **Neutral:** the decision does not define or implement GitRepository authority cutover; old-writer exclusion and activation remain separate work.

## References

- ADR-032: GitRepository is hidden infrastructure; Project is retired.
- `MIGRATION_065`
- `DormantGitRepositoryTargetBinding`
- `tests/e2e/git_repository_r1_compat.py`
- `specs/git-repository/requirements.md`
- `specs/git-repository/git-repository.allium`
