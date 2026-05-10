# Tasks UI — Executive Summary

## Scope and Boundary

This spec governs the user-facing **task list and task viewer** inside a conversation: the collapsible Tasks panel that surfaces the repository's `tasks/` directory, and the single-task detail view that opens when I click into one.

The task system itself — the `tasks/<NNNNN>-pX-status--slug.md` filename grammar, the `taskmd new` CLI, the `./dev.py tasks validate` rules, the frontmatter schema — is not spec'd in `specs/`; it's documented in `AGENTS.md` and enforced by tooling. This UI spec consumes those files but does not define them.

**In scope here (user-facing experiences):**
- Seeing the tasks tracked for this conversation's repo without leaving the page
- Recognising the task this conversation is working on, if any
- Drilling into a task's content and metadata
- Starting work on a task as a seeded sub-conversation
- Navigating between a task and the conversation that's working on it
- Deferring task loading until I actually look (don't pay the cost on every conversation mount)

**Owned by other specs:**
- `AGENTS.md` — the task file format (`NNNNN-pX-status--slug.md`), the `taskmd` CLI, the `./dev.py tasks` workflow, the priority/status enums
- `specs/seeded-conversations/` — the "Start working" hand-off (REQ-SEED-001..004 cover the seeded conversation + draft prefill)
- `specs/conversation-ui/` — the parent layout that hosts the Tasks panel
- `specs/projects/` — the conversation/branch lifecycle that "Go to conversation" navigates into

## Why It Exists

The agent and I both work against the `tasks/` directory: the agent creates and updates task files via `taskmd`, I review and triage them. Without a UI surface, I'd have to leave the conversation, navigate the file tree, and read raw markdown. The Tasks panel makes the queue visible inline; the TaskViewer makes one-click "okay, work on this next" a real action.

## Status Summary

| Requirement | Status | Notes |
|---|---|---|
| **REQ-TASKS-UI-001:** See My Tasks Without Leaving the Conversation | ✅ Complete | `ui/src/components/TasksPanel.tsx:34-110` (collapsible header + list); count summary on the header after first expansion satisfies the spec text (the future count-only endpoint is explicitly out of scope per requirements.md rationale); empty-state copy is now the neutral `"No tasks found"` (`:117`) covering directory-missing / empty / errored alike |
| **REQ-TASKS-UI-002:** Tasks Grouped by Status with Active/Closed Distinction | ✅ Complete | `:15-22` (status order), `:32` (terminal-status set), `:67-77` (group + sort), `:114-172` (per-group render with terminal-collapsed-by-default) |
| **REQ-TASKS-UI-003:** Recognize My Current Task at a Glance | ✅ Complete | `:135` (currentTaskId match), `:142,152` (highlight + "current" badge) |
| **REQ-TASKS-UI-004:** Open a Task's Full Details | ✅ Complete | `:145` (onTaskClick) wires into `TaskViewer.tsx:114-190` (status + priority + slug + filename + raw content with frontmatter stripped) |
| **REQ-TASKS-UI-005:** Start Working on a Task as a Seeded Sub-Conversation | ✅ Complete | `TaskViewer.tsx:79-112` (seeded-conversation creation), `:202-215` (prompt builder); cross-references `specs/seeded-conversations/` REQ-SEED-001..004 |
| **REQ-TASKS-UI-006:** Navigate Between a Task and Its Conversation | ✅ Complete | `TasksPanel.tsx:153-164` (→ button on task row when `conversation_slug` is present), `TaskViewer.tsx:162-172` ("Go to conversation" link) |
| **REQ-TASKS-UI-007:** Pay the Load Cost Only When I Look | ✅ Complete | `TasksPanel.tsx:53-58` clears `tasks` immediately on `conversationId` change so a stale count from the prior conversation never bleeds across navigation; `:60-72` keeps the load itself gated on `expanded` so the round-trip is paid only when the user looks |

**Progress:** 7 of 7 complete.

## Behavioural Specification

No Allium spec — the panel and viewer are read-mostly views with local UI state (expanded/collapsed group, loading flag, seed-in-flight) and no meaningful state machine. The "Start working" flow has lifecycle, but it's owned by `specs/seeded-conversations/` (REQ-SEED-001 through -004); this spec just describes the trigger.

## Cross-Spec Cross-References

- `AGENTS.md`: the task file grammar, `taskmd new` CLI, status/priority enums, `./dev.py tasks fix/validate`. The Tasks panel reads what `taskmd` writes — never the other way around.
- `specs/seeded-conversations/`: the "Start working" button hands off via the same seeded-conversation mechanism the terminal panel's "Let Phoenix set this up" CTA uses (REQ-TPANEL-009). Failures in seed creation surface inline as `seedError` in the TaskViewer header (`TaskViewer.tsx:35,108-111,136-138`).
- `specs/conversation-ui/`: the Tasks panel sits inside the conversation page's right-rail / settings area. Layout is owned there; this spec governs the panel's content.
- `specs/projects/`: the `conversation_slug` field on a task entry comes from the project/conversation linkage layer; clicking through navigates via standard React Router.
