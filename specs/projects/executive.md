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

## Normative Authority

Use `specs/projects/requirements.md` only as a retired index. Normative repository-backed requirements now live in the destination specs it lists. `projects.allium` is retired and deleted; its surviving behavioral rules now live in `specs/git-repository/git-repository.allium`, `specs/bedrock/bedrock.allium`, `specs/work-lifecycle/work-lifecycle.allium`, and `specs/conversation-creation/requirements.md` for the root-floor seam.
