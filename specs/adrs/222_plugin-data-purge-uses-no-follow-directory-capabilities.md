# ADR-222: Plugin data purge uses no-follow directory capabilities

- **Status:** Accepted
- **Date:** 2026-11-03
- **Affects:** REQ-AP-006, REQ-AP-007, REQ-AP-009, REQ-AP-010

## Context

A metadata check followed by pathname-based recursive removal could traverse an
intermediate data-root symlink installed by another harness between the check
and deletion. Late package-absence checks alone did not bind filesystem authority.

## Decision

Open the trusted resolved user root, its data directory and the requested target
with directory/no-follow flags. The requested name is a validated PluginName.
Return an opaque, one-use deletion capability containing parent and target file
descriptors, not a path that the caller can re-resolve.

Walk descendants relative to held descriptors. Open directories with no-follow;
unlink child symlinks without traversing them. Before removing each directory
entry, compare its no-follow identity with the opened directory. Use an explicit
stack rather than recursive Rust calls. Resource or I/O failure retains registry
identity and reports the partial outcome. Missing roots/data remain valid absence;
unsupported platforms get no unsafe pathname fallback.

Use the existing Rustix dependency for safe descriptor-relative operations rather
than new unsafe syscall glue or a general lifecycle framework. Trusted root aliases
remain supported; data-path symlinks below that root do not.

## Options considered

Rechecking strings narrows a race but does not bind the next syscall. A mutable
lock cannot constrain independent harnesses. A path under a process descriptor
pseudo-filesystem would depend on host mount behavior. Bound directory operations
express the intended authority directly on the supported Unix hosts.

## Consequences

Directory/file replacement can cause a reported partial deletion, but cannot
redirect traversal through a replacement data symlink. This is not a transaction
across independent writers or a filesystem sandbox for all user operations.

Client command admission also recognizes Windows path separators and drive/stream
syntax rather than treating them as bare executables. Server identifiers with the
reserved `__` tool delimiter are skipped instead of advertising unresolvable tools.
Present non-regular SKILL.md entries receive skipped outcomes; absent entries are
still not skills. These restrictions are reported at their own failure scope.
