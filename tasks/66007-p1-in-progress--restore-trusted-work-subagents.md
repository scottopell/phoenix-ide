# Restore trusted Git-backed Work delegation

User approved 2026-09-28: remove filesystem sandboxing from write subagents entirely; retain Explore sandboxing. This supersedes the original isolation goal split from PR #765. ADR-068 records the decision.

## Acceptance

- Git-backed parents with approved Work authority admit Work children in the existing parent worktree and exact durable WorkScope.
- Work children use ordinary unsandboxed Bash, patch, and configured MCP tools. No sandbox platform admission, scratch confinement, private filesystem, or edit-transfer architecture.
- Children cannot own parent approval/lifecycle/worktree cleanup. Explore remains read-only and sandboxed; Direct Work remains available.
- Admission, persistence, reconstruction, tool access, and ownership tests pass; full checks and exact-head Codex review are clean.

## Preserved evidence

The superseded implementation is retained at af7d503e568ab0009caf8392cf4951356e5caf56 (local archive/pr765-child-isolation-af7d503). The macOS hardlink probe disproved absolute path-based write confinement. That guarantee is deliberately not part of the trusted-worker contract.

Deployment, deflake recovery, and coordinator usage work remain separately tracked.
