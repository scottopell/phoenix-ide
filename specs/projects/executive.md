# Projects — Executive Summary

## Status

`specs/projects/` is retired as a normative authority. `Project` no longer owns repository identity, product surfaces, lifecycle, or API behavior. This directory remains only as a traceability waypoint so older REQ-PROJ references still resolve deterministically during the staged migration.

## Current Reality

The shipped codebase still contains many `project`-named implementation surfaces, endpoints, and persistence fields. That current-reality drift is intentionally documented in the destination executives (`git-repository`, `conversation-creation`, `bedrock`, `conversation-ui`, `file-explorer`, `work-lifecycle`, `global-recall`, `ios_client`) rather than here.

## What Changed in This Spec Set

- Repository-backed hidden infrastructure moved to `specs/git-repository/`
- Conversation-creation rules moved to `specs/conversation-creation/`
- Task approval and execution-authority rules moved to `specs/bedrock/`
- Sub-agent authority inheritance moved to `specs/subagents/`
- Platform capability detection moved to `specs/bash/`
- Conversation-list filtering moved to `specs/conversation-ui/`
- Live checkout diff grounding moved to `specs/file-explorer/`
- Restart repair and WorkScope ownership moved to `specs/work-lifecycle/`

## Requirement Redirect Ledger

`specs/projects/requirements.md` contains only canonical standing deprecations for Project-specific behavior. Moved requirements are canonically declared by these owners:

## Moved REQ-PROJ IDs

| REQ ID | New owner |
|---|---|
| REQ-PROJ-000 | `specs/conversation-creation/requirements.md` |
| REQ-PROJ-001 | `specs/conversation-creation/requirements.md` |
| REQ-PROJ-001A | `specs/conversation-ui/requirements.md` |
| REQ-PROJ-002 | `specs/conversation-creation/requirements.md` |
| REQ-PROJ-003 | `specs/bedrock/requirements.md` |
| REQ-PROJ-004 | `specs/bedrock/requirements.md` |
| REQ-PROJ-005 | `specs/conversation-creation/requirements.md` |
| REQ-PROJ-005A | `specs/conversation-creation/requirements.md` |
| REQ-PROJ-006 | `specs/bedrock/requirements.md` |
| REQ-PROJ-007 | `specs/bedrock/requirements.md` |
| REQ-PROJ-008 | `specs/subagents/requirements.md` |
| REQ-PROJ-012 | `specs/bedrock/requirements.md` |
| REQ-PROJ-013 | `specs/bash/requirements.md` |
| REQ-PROJ-015 | `specs/git-repository/requirements.md` |
| REQ-PROJ-017 | `specs/conversation-creation/requirements.md` |
| REQ-PROJ-018 | `specs/bedrock/requirements.md` |
| REQ-PROJ-019 | `specs/conversation-ui/requirements.md` |
| REQ-PROJ-020 | `specs/git-repository/requirements.md` |
| REQ-PROJ-021 | `specs/git-repository/requirements.md` |
| REQ-PROJ-022 | `specs/conversation-creation/requirements.md` |
| REQ-PROJ-024 | `specs/git-repository/requirements.md` |
| REQ-PROJ-025 | `specs/git-repository/requirements.md` |
| REQ-PROJ-028 | `specs/conversation-creation/requirements.md` |
| REQ-PROJ-028a | `specs/work-lifecycle/requirements.md` |
| REQ-PROJ-029 | `specs/conversation-creation/requirements.md` |
| REQ-PROJ-033 | `specs/bedrock/requirements.md` |
| REQ-PROJ-036 | `specs/bedrock/requirements.md` |
| REQ-PROJ-038 | `specs/file-explorer/requirements.md` |
| REQ-PROJ-WS-001 | `specs/work-lifecycle/requirements.md` |

`projects.allium` is retired and deleted; its surviving behavioral rules live in `specs/git-repository/git-repository.allium`, `specs/bedrock/bedrock.allium`, `specs/work-lifecycle/work-lifecycle.allium`, and the owning requirements files.
