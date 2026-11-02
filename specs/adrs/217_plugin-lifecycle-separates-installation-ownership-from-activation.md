# ADR-217: Plugin lifecycle separates installation ownership from activation

Date: 2026-11-02
Status: Accepted

## Context

Adversarial review of the user-level plugin implementation found that a
canonical package path was also used as an uninstall target. Registry errors
could enable disabled plugins. Removing the registry row made retained data
invisible, and unchecked copy publication could modify an existing entry.
ADR-216 established scope and naming but did not settle these boundaries.

## Decision

- Keep the user-level installation entry separate from the canonical package
  execution root. Uninstall unlinks a symlink entry; it never deletes its target.
- Persist the known installation entry with registry identity and provenance.
  A malformed manifest is not proof that the entry is absent. Unknown locations
  do not authorize data purge.
- Removal records disabled identity before deleting the entry. Retain identity
  and data until explicit purge. Reinstall registers managed/enabled identity
  without resetting data.
- SQLite is the durable activation authority. One instance-scoped runtime owner
  shares a stable snapshot with skills and MCP. An uninitialized snapshot permits
  no plugins. A failed refresh preserves the last successful snapshot.
- Serialize lifecycle mutations, snapshot publication, and MCP reconciliation
  within one instance. Other harnesses remain independent.
- Stage and validate copied files before exclusive, no-replace publication.
  Registration failure after publication is an explicit partial failure, not
  success or an automatic rollback promise.
- Resolve the trusted user-level root before destructive data operations and
  reject symlinks below that data boundary. Root aliases remain usable.

## Alternatives

A process-global disabled set plus an MCP-local set allowed divergent decisions
and cross-instance interference. Using only a canonical package path erased
installation ownership. Removing registry identity at uninstall erased the
handle needed for safe retained-data operations. A generic lifecycle framework
would add machinery without changing these boundaries.

## Consequences

Plugin-format parsing remains independent of activation. Parsed candidates are
not presented as execution-ready components: skill catalog admission and MCP
preparation retain structured rejection reasons. Package-relative commands are
resolved independently of cwd and distinguished from PATH lookups.

Filesystem, SQLite, and runtime changes are not one transaction. Errors report
partial outcomes, and failed reads never fabricate all-enabled state. This
choice adds no crash recovery, prototype-database conversion, rollback, or
cross-harness locking guarantee. Existing duplicate-name discovery policy still
applies when other harnesses publish distinct entries with the same identity.
