# ADR-292: Bind creation pins to the supplying Git common-directory incarnation

- **Status:** Accepted
- **Date:** 2026-11-27
- **Affects:** REQ-CCR-005A; conversation creation Git aggregate staging

## Context

A durable creation pin currently records the commit and canonical branch selected from a Git common directory. The common-directory path can later name a different clone containing the same commit. Without evidence binding the pin to the original directory incarnation, a resumed worker could materialize the old pin in a replacement repository.

REQ-GITREP-001 defines a separate hidden, opaque Phoenix-local `GitRepository` identity. Creation needs filesystem evidence to bind one immutable pin to the particular common-directory instance that supplied it; this does not mint, replace, or redefine that durable repository identity.

## Options considered

- Use the checkout OID and locator alone: cannot distinguish a replacement clone.
- Write a new identity marker in the source repository: introduces extra source mutations and can be copied with repository contents.
- Bind the pin to filesystem incarnation evidence without writing a marker in the source repository.

## Decision

Persist a local filesystem-incarnation token with the exact starting pin in the same durable write. Establish it from the common directory's device, inode, and creation/birth time. Linux must obtain the birth time from an explicit `statx` observation; macOS uses the filesystem creation time (`created()`). If the required fields or timestamp are unavailable, reject pin resolution. Do not fall back to path strings, commit OIDs, GitRepository identity, ctime, mtime, or another timestamp.

At the Git aggregate staging boundary, require current evidence to match the pin. Apply the same binding when adopting staging and publishing. Reject staging on missing, unavailable, or mismatched evidence without rewriting the pin. This is a Git-backed aggregate guard only: it does not gate Direct creation or objective delivery.

A historical pin without incarnation evidence is never backfilled from the repository now at its locator, and is permanently unusable for new staging. If materialization may already have occurred, preserve the checkout and registration and retain the job as ambiguous; do not remove or replace resources based on an unbound pin. If materialization could not have occurred, reject the attempt without selecting a new pin or falling back to Direct.

The Phoenix repository lock coordinates Phoenix workers only. Filesystem identity observations are not an exclusion guarantee against arbitrary concurrent external path swaps and do not establish a general repository identity service.

## Consequences

Git-backed creation requires filesystems that expose the specified incarnation evidence; unsupported or incomplete observations fail closed. A replacement clone at the same locator cannot inherit the original pin merely by containing the same OID. Historical unbound jobs may require manual resolution if materialization is possible. REQ-GITREP-001 and its hidden identity semantics remain unchanged.

## References

- `specs/conversation-creation/requirements.md` — REQ-CCR-005A
- `specs/conversation-creation/conversation-creation.allium`
- `specs/git-repository/requirements.md` — REQ-GITREP-001 (distinct identity)
- ADR-291
