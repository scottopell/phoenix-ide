# ADR-068: Work children share the parent's development trust

- **Status:** Accepted
- **Date:** 2026-09-28
- **Affects:** REQ-PROJ-008, REQ-BED-018, Bash write-capability policy

## Context

PR #765 separated parent approval repair from Git-backed Work-child execution after a macOS probe showed a writable worktree hardlink could modify an outside inode. Phoenix serves one primary user whose heavy delegation needs normal development tools. The user explicitly chose trusted write workers and retained sandboxing only for Explore mode.

## Options considered

1. **Hostile-worker containment:** private filesystems and edit transfer could establish a stronger boundary, but add lifecycle and integration machinery without an established need.
2. **Path sandbox for Work children:** reduces accidental writes but cannot promise inode isolation and complicates tool, cache, and MCP access.
3. **Trusted Work children:** ordinary development tools in the parent's existing environment, with separate application lifecycle authority.

## Decision

Choose trusted Work children. This supersedes ADR-067's deferred child-isolation requirement, not its parent capability projection. Work children use ordinary unsandboxed Bash, patch, and configured MCP tools. Git-backed children attach to the exact parent WorkScope and carry a non-owning mode. Direct delegation is preserved. Explore children retain their read-only Bash sandbox and omit Bash if the platform cannot enforce it.

The parent owns task approval, worktree lifecycle, and integration. Starting-directory validation and assignment partitioning are coordination mechanisms, not security boundaries. Child Bash can run Git and access files outside the worktree; normal host permissions apply.

## Consequences

- Delegation works without host sandbox support for write operations.
- Children do not receive parent lifecycle or recursive-delegation tools.
- Trusted workers can interfere with host files and sibling edits; no filesystem, credential, network, or adversarial containment guarantee is made.
- Shared-worktree collaborators must preserve unrelated edits and report conflicts.

## References

- ADR-067
- `RuntimeManager::sub_agent_child_mode`
- `specs/subagents/requirements.md`
- `specs/bash/requirements.md`
