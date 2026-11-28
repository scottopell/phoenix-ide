# ADR-295: Keep the conversation lifecycle detached and local

- **Status:** Accepted
- **Date:** 2026-11-26
- **Affects:** REQ-CCR-005, REQ-PROJ-005, REQ-TAT-001, REQ-BED-028; conversation creation, task approval, and worktree custody

## Context

Conversation creation and approval had accumulated user-visible lifecycle choices and side effects: branch/managed/automatic modes, new-conversation task handoffs and forks, Git mutations while approving a task, ignore-file writes, permanent deletion, and startup orphan reclamation. These behaviors expanded the resource lifecycle beyond the ordinary Phoenix-owned detached worktree and made an approval action change repository state.

## Decision

A Git-backed conversation uses one Phoenix-owned disposable worktree at detached `HEAD`, pinned to the exact authoritative starting commit. Creation does not create or select a branch, write a commit, or modify ignore configuration. Direct conversations remain supported when there is no Git repository; Direct mode and sub-agent execution remain supported.

Task approval is a same-conversation checkpoint: it approves the exact reviewed task snapshot and grants the existing conversation's existing `WorkScope` authority. Approval does not rename, stage, commit, or otherwise mutate Git state, create a worktree or conversation, or hand execution to a new conversation. New-conversation approval placement handoffs, fork proposals, Branch/Managed/Auto lifecycle modes, and permanent Delete are not offered.

The system does not scan for arbitrary orphaned resources at startup or run generic reclamation. It still reconciles recorded creation jobs and validates environments when used. The explicit Close flow remains governed by its existing safety requirements; this decision does not weaken Close inspection, blockers, retention on uncertainty, retry, or loss reporting. Context-exhaustion continuation is a separate existing behavior and is not changed by this decision.

## Consequences

The normative contracts are recorded in `specs/conversation-creation/requirements.md`, `specs/bedrock/requirements.md`, `specs/task-approval-tools/requirements.md`, `specs/git-repository/requirements.md`, `specs/work-lifecycle/requirements.md`, and `specs/simple_client/requirements.md`. This decision supersedes ADR-043’s repository-root staging placement and its use of a private namespace and cooperating lock as sufficient rollback custody. The source-snapshot and receipt boundary in ADR-287 still applies without an approval-time Git mutation. ADR-207’s historical image-retention rules do not authorize a new permanent Delete. Retire pending fork proposals by marking them dismissed without deleting proposal snapshots, child or source conversations, provenance, or resources. Existing Git modes remain historical provenance, not alternate creation options.

The product retains conversation resources except through explicit safe Close and exact identity-owned rollback of unpublished creation. A job token and a Phoenix repository lock do not exclude external writers. Once Git materialization may have begun, the worker retains the checkout and registration unless it can prove safe rollback; the ordinary Git staging path supplies no such exclusive custody proof. Failure or cancellation records ambiguity and does not trigger later reclamation. About → Disk remains read-only; it does not offer a cleanup action or endpoint. There is no automatic startup cleanup path. Integrations must reject unsupported placement or lifecycle requests rather than silently creating a separate conversation or Git branch.

## Options considered

- Keep multiple Git lifecycle modes and require users to choose per conversation: rejected because the supported workflow is one detached disposable worktree.
- Preserve new-conversation approval handoff and forks: rejected because approval is a checkpoint on the same conversation and work scope.
- Reclaim orphaned resources on startup or add permanent Delete: rejected because resource reclamation is not part of this lifecycle contract and could bypass explicit Close safety.
- Retain approval commits or ignore-file edits: rejected because task approval and conversation setup must not mutate Git state beyond ordinary user-authorized work.

## References

- `specs/conversation-creation/requirements.md`
- `specs/bedrock/requirements.md`
- `specs/task-approval-tools/requirements.md`
- `specs/git-repository/requirements.md`
- `specs/work-lifecycle/requirements.md`
- `specs/simple_client/requirements.md`
