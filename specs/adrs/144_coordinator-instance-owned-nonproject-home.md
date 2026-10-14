# ADR-144: The Coordinator owns a local non-project home through an explicit WorkScope

- **Status:** Accepted
- **Date:** 2026-10-14
- **Affects:** REQ-GR-006, REQ-GR-007, REQ-GR-013, REQ-GR-016–018; Coordinator home initialization and retention

## Context

Global borrows ordinary workers' scopes to stage evidence. That makes retained files depend on disposable worktree lifecycles and does not provide an authoritative home during zero-worker operation. The local home requirement was extracted from the federation design; all remote peer protocol, notes and subscription scope remain federation-owned. The original design is pinned at `4e67257292f09823d3ac76622c404c0021c57dcf` (REQ-FED-011 and the home-grounding portion of REQ-FED-012). Its historical ADR-073 belongs to that commit-specific design, not the unrelated ADR with that number on this branch.

## Options considered

1. Continue borrowing worker cwd or create a fake repository/project: cheap, but ownership and retirement are wrong and Git semantics are fabricated.
2. Add an implicit default cwd, as the historical federation draft proposed: convenient but conflicts with the actual explicit active WorkScope tool boundary and obscures selected project authority.
3. Introduce an instance-owned singleton binding and explicit non-project environment, retaining mandatory explicit tool targeting: creates migration and reconciliation work, but preserves existing local tool contracts and later federation integration.

## Decision

Choose option 3. Persist one `coordinator_work_scope` relation to `work_scopes`; its authoritative path is only the environment cwd. Attach Coordinator chain members to it, never ordinary product ownership. `CoordinatorHome` is distinct from an allocated worktree or unowned cwd. Initialize beneath `PhoenixRuntimeEnvironment::data_dir()` as `coordinator/`, canonicalize once, reject symlink/non-directory/mismatched or worker-overlapping ownership, and refuse to recreate missing bound state automatically.

SQL constraints and API fences prevent ordinary environment mutation, retirement and deletion. Scoped Bash and SVG retain explicit active `work_scope_id`; no remote cwd execution or scope-sharing shortcut is introduced. Stable home metadata is generated from persisted authority, not accepted from notes or a handoff. Prompts preserve deliberately selected historical pointers without ingesting files. Server-backed desktop/mobile browsing uses the existing contained file viewer, not host-local OS actions or a Git work view.

Ordinary routing resources stay distinct from Coordinator process routing. A persistent home does not imply process handles survive restart. Development defaults isolate data roots as well as DBs; operator configuration remains authoritative.

## Consequences

- **Positive:** zero-worker execution, stable retained bytes across continuation/restart, honest non-project grounding, and shared explicit scope contracts for later federation.
- **Negative:** SQLite environment-shape constraints require a transactional table rebuild preserving schema objects and foreign keys; bound-path mismatch/missing bytes require operator intervention rather than convenient destructive repair.
- **Neutral:** the home organizes unsandboxed local execution; it is not a security sandbox or generic memory platform. Notes confer no authority. Conversation/database-only export does not include home bytes; manual paired offline backup is required for complete state.

This partially supersedes ADR-026/ADR-047 only insofar as they exclude Coordinator instance-owned filesystem scope; ordinary ProductConversation/worktree ownership and explicit target authorization remain unchanged. It supersedes the implicit default-Bash-target aspect of the pinned federation home design, not any remote requirement.

## References

- `specs/global-recall/requirements.md` REQ-GR-016–018
- `specs/compatibility/requirements.md` offline replacement boundary
- `Database::initialize_coordinator_work_scope`, `coordinator_home::initialize`, `GlobalReadService::resolve_active_work_scope_bash_target`
