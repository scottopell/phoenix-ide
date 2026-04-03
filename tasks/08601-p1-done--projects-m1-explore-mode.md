---
created: 2026-03-05
priority: p1
status: done
artifact: completed
---

# Projects M1: Project Entity + Explore Mode

## Summary

Establish the Project as a first-class entity and make Explore mode the default for
all conversations. This is the foundation milestone — every subsequent milestone
builds on it.

## Context

Read first:
- `specs/projects/requirements.md` — REQ-PROJ-001, REQ-PROJ-002, REQ-PROJ-013, REQ-PROJ-014
- `specs/bedrock/requirements.md` — REQ-BED-027 (ConvMode field)
- `specs/bedrock/design.md` — "Conversation Mode" section

## What to Do

### Backend

1. **Project entity in DB:** Add a `projects` table keyed by resolved repo root
   path. When a conversation is created, resolve the cwd to its git repo root
   (trace worktrees back to main checkout). Create or find the project. Associate
   the conversation with it.

2. **ConvMode field:** Add `conv_mode TEXT NOT NULL DEFAULT '"Explore"'` to the
   conversations table. All existing conversations get Explore mode.

3. **Platform capability detection (REQ-PROJ-013):** On startup, probe for Landlock
   (Linux) and sandbox-exec (macOS). Store as an in-memory enum:
   `PlatformCapability::None | Landlock | MacOSSandbox`

4. **Tool registry by mode (REQ-PROJ-002):**
   - Explore + no sandbox: ReadFile, Search, Think, keyword_search, read_image,
     spawn_agents, browser tools. No bash, no patch.
   - Explore + sandbox available: all tools with bash sandboxed read-only.
   - Work mode: all tools (for future milestones).
   - Return clear error when blocked tools are called: "Write tools are disabled
     in Explore mode. Use create_task to propose work requiring write access."

5. **ReadFile and Search tools:** Create these new tools for the no-sandbox Explore
   case. ReadFile reads a file by path. Search does text/regex search across files
   (similar to keyword_search but filesystem-level, no embeddings needed).

6. **API endpoints:** `GET /api/projects` — list projects with conversation counts
   and active task counts.

### Frontend

7. **Project switcher (REQ-PROJ-014):** Tabs at the top of the sidebar. Each tab is
   a project. Selecting a tab filters the conversation list.

8. **Mode indicator:** Each conversation shows Explore/Work badge.

## Acceptance Criteria

- [ ] New conversations auto-detect their project from git repo root
- [ ] Two conversations in the same repo share a project
- [ ] Explore mode conversations cannot call bash or patch (no-sandbox case)
- [ ] Calling bash in Explore returns actionable error mentioning create_task
- [ ] Platform capability detected correctly on startup
- [ ] ReadFile and Search tools work in Explore mode
- [ ] Project tabs appear in sidebar
- [ ] `./dev.py check` passes

## Value Delivered

Every conversation starts safe. No accidental writes. This replaces the abandoned
Landlock-only approach (old REQ-BED-014) with a cross-platform solution.
