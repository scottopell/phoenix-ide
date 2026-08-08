# ADR-027: Work sub-agents share a concurrent write boundary

- **Status:** Accepted
- **Date:** 2026-08-07
- **Affects:** REQ-SA-001, REQ-PROJ-008, REQ-BED-018, `SubAgentSpecsResolved`

## Context

Phoenix lets a write-capable parent delegate implementation to Work sub-agents.
Each child is an independent conversation and runtime, but it inherits the
parent's writable environment and attached `WorkScope`. Parent fan-out, result
buffering, cancellation, timeout, and fan-in already support multiple pending
children without depending on the children's execution authority.

The spawn layer nevertheless admits at most one active Work child per parent and
at most one Work task per call. That preserves a single writer but prevents a
parent model from reducing latency by assigning independent implementation tasks
to concurrent children. The limit also adds runtime-only reservation state whose
lifecycle must be maintained through completion and forced cancellation.

Concurrent children can contend if their assignments overlap. File edits may
observe stale content, Git operations may conflict, and one child can observe an
intermediate state produced by another. Avoiding every such interaction would
require serialization, separate worktrees plus merge orchestration, or a new
cross-runtime locking protocol.

## Options considered

1. **Retain one active Work child per parent** — prevents child-child write
   contention and keeps the worktree single-writer, but forces independent work
   to run serially and leaves the parent model unable to use Work fan-out.
2. **Give every Work child a private worktree and merge the results** — isolates
   writes, but introduces branch allocation, dependency ordering, merge conflict
   handling, cleanup, and another Git lifecycle beneath the parent conversation.
3. **Allow concurrent Work children in the shared parent environment** — uses
   the existing child identity and fan-in model, maximizes useful parallelism,
   and accepts ordinary filesystem, patch, process, and Git conflicts when task
   decomposition is poor.

## Decision

Choose option 3. An eligible parent may spawn multiple write-capable sub-agents
concurrently, both within one `spawn_agents` call and across multiple calls in
one tool round. Write-capable children continue to inherit the parent's writable
environment and attached `WorkScope`. The per-call limit of ten tasks and every
existing parent-authority, model, cwd, timeout, cancellation, and result-fan-in
rule remain in force.

The parent model owns task decomposition. Phoenix does not infer file ownership,
serialize writable tools, allocate child worktrees, or merge child branches.
Overlapping work retains the existing patch, bash, filesystem, and Git behavior,
including visible errors or conflicting writes. The worktree is the structural
write boundary, not a single-writer resource.

## Consequences

- **Positive:** Independent implementation tasks can complete concurrently, and
  `spawn_agents` has the same parallel meaning for read-only and write-capable
  children.
- **Positive:** The executor no longer needs an `active_work_subagents`
  reservation counter or cancellation-specific reservation release logic.
- **Positive:** Existing pending-child, out-of-order result, timeout,
  cancellation, and fan-in machinery applies without a mode-specific admission
  exception.
- **Negative:** Poorly decomposed children can edit the same file, run
  conflicting Git commands, or observe sibling intermediate state.
- **Negative:** Phoenix provides no automatic conflict reconciliation; the
  parent must inspect child outcomes and repair conflicts when they occur.
- **Neutral:** WorkScope attachment, cwd containment, and cleanup ownership do
  not change.

## References

- `ConversationRuntime::handle_spawn_agents_tool`
- `RuntimeManager::handle_spawn_request`
- `handle_core_tool_complete`
- `specs/subagents/subagents.allium`
- `specs/bedrock/bedrock.allium`
- `specs/projects/projects.allium`
