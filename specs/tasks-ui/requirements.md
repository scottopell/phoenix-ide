# Tasks UI

## Scope

This spec governs the **Tasks panel** and the **Task viewer** inside a conversation: the surface that lists the repo's `tasks/` directory and the detail view that opens when I click in. The task file format, the `taskmd` CLI, and the `./dev.py tasks` validation are out of scope here — they're documented in `AGENTS.md`. This spec just covers the UI layer that reads those files. See `specs/tasks-ui/executive.md` for the full boundary.

## User Story

As a Phoenix user inside a conversation, I want to see what tasks exist for this repo without leaving the page, recognise which one (if any) this conversation is working on, drill into a task's content, and start a fresh sub-conversation pre-filled with the task body — all without context-switching to the file tree or remembering CLI commands.

## Transparency Contract

The Tasks UI must let me confidently answer:

1. What tasks are tracked for this repo?
2. Which are in-progress, ready, blocked? Which are done?
3. What's the priority of each?
4. Which task — if any — is _this_ conversation working on?
5. Does this task already have a conversation? Where?
6. How do I start working on a ready task?
7. Did my "Start working" actually create the new conversation?

Each numbered question maps to one or more requirements below.

---

## Requirements

### REQ-TASKS-UI-001: See My Tasks Without Leaving the Conversation

WHEN I'm inside a conversation
THE SYSTEM SHALL show a collapsed "Tasks" panel header in the conversation's chrome, carrying a count summary (e.g. "Tasks · 3 active · 7 closed") from first paint

WHEN I click the header
THE SYSTEM SHALL expand the panel and load the full task list

WHEN the response is empty (no tasks found, OR `tasks/` doesn't exist, OR the request errored)
THE SYSTEM SHALL show a brief, non-alarming empty-state message rather than a loud error banner

**Rationale:** The task list is reference data, not a control surface I'm always interacting with. The collapsed header carries its count summary on first paint via a lightweight count endpoint — just the status tallies, no full list — so the summary is visible without expanding while the expensive full-list read stays deferred until I look (see REQ-TASKS-UI-007).

The empty-state condition is deliberately broad. The UI shows neutral `"No tasks found"` copy whenever the response is empty (`TasksPanel.tsx:117`) — covering all three situations: (a) the directory doesn't exist, (b) it exists and is empty, (c) the request failed and the error was logged to console without surfacing. The REQ accepts this ambiguity: empty-state-as-non-error is the right baseline, and a future tightening could differentiate the three only if the API starts distinguishing them.

---

### REQ-TASKS-UI-002: See Tasks Grouped by Status, Active Above Closed

WHEN the task list is loaded
THE SYSTEM SHALL group tasks by status: `in-progress`, `ready`, `blocked`, `brainstorming`, `done`, `wont-do`

WHEN displaying groups
THE SYSTEM SHALL render active groups (`in-progress`, `ready`, `blocked`) expanded by default and terminal groups (`done`, `wont-do`) collapsed by default

WHEN displaying a task row
THE SYSTEM SHALL show its priority badge (`p0`..`p4`), task ID, slug, and — if the task has an associated conversation — a → arrow to navigate to it

**Rationale:** Active work is the headline; closed tasks are reference. Defaulting terminal groups to collapsed keeps the panel scannable without losing the history. Priority is at the front of every row because filter-by-priority is a common scan ("anything p0?"). Grouping by status (vs sorting flat by priority + status) matches how I think about my queue: "what's in flight?" then "what's ready to pick up?" then "what's blocked?"

---

### REQ-TASKS-UI-003: Recognize My Current Task at a Glance

WHEN this conversation is associated with a task (Work mode tracks one)
THE SYSTEM SHALL render that task row with a visual highlight and a "current" badge
AND SHALL NOT render the → conversation arrow on it (it's already this conversation)

**Rationale:** Work-mode conversations are scoped to one task by design. Visually anchoring "this is the one you're doing" prevents the very reasonable "wait, which one was I on?" scroll-back I'd otherwise do. Suppressing the → arrow on the current row is the small detail that says "you're already here."

---

### REQ-TASKS-UI-004: Open a Task's Full Details

WHEN I click a task row
THE SYSTEM SHALL show me the task viewer with:
- Task ID (header)
- Status (with a colour cue matching the status group)
- Priority
- Slug
- Filename
- Conversation link, if associated
- Full task content with YAML frontmatter stripped

WHEN the file fails to load
THE SYSTEM SHALL show the error inline in the Content section without losing the rest of the task metadata

WHEN I want to go back
THE SYSTEM SHALL provide an obvious back button that returns me to the panel list view

**Rationale:** The same metadata I see in the row, plus the body the agent and I read when triaging. Stripping frontmatter on render keeps the visible content focused on the body — the metadata is rendered separately above. A failed file read should not blank the whole viewer; the row's worth of info is still useful even when the body isn't reachable.

---

### REQ-TASKS-UI-005: Start Working on a Task as a Seeded Sub-Conversation

WHEN I'm viewing a non-terminal task (status is not `done` or `wont-do`)
AND I'm in the context of a parent conversation
THE SYSTEM SHALL show a "Start working" button in the task viewer header

WHEN I click "Start working"
THE SYSTEM SHALL create a new conversation in the parent's working directory, seeded as a child of the current conversation, and pre-fill the new conversation's input with a prompt that contains the full task body and asks the agent to read the scope and execute carefully (see `specs/seeded-conversations/`)

WHEN the seed-conversation creation fails
THE SYSTEM SHALL surface the failure visibly in the task viewer header (not only via `console.error`), so I'm not left wondering whether anything happened

WHEN the task content is still loading
THE SYSTEM SHALL disable the "Start working" button with a clear "loading" tooltip — clicking before the body is loaded would produce an empty seed prompt

**Rationale:** "Start working" is the load-bearing action of this whole UI: it turns "I see this task in the panel" into "I am now executing this task." The seeded conversation pattern (review-first prompt, no auto-submit) means I see exactly what's about to happen before I commit. Visible-on-failure error surfacing matches the lesson from the terminal panel: console-only errors leave me wondering whether the click did anything. Disabling on partial load avoids silent empty-prompt seeds.

---

### REQ-TASKS-UI-006: Navigate Between a Task and Its Conversation

WHEN a task in the list has an associated conversation
THE SYSTEM SHALL render a → button on the task row that, when clicked, navigates to that conversation

WHEN a task in the viewer has an associated conversation
THE SYSTEM SHALL render a "Go to conversation" link in the Details section

WHEN I'm already viewing the associated conversation (REQ-TASKS-UI-003 currentTaskId match)
THE SYSTEM SHALL NOT render the → button on the row (the "current" badge already says "you're here")

**Rationale:** Tasks and conversations have a 1:1 mapping in Work mode. Bidirectional navigation makes the queue feel like a workspace rather than a static list. The → suppression on the current row prevents the user from clicking and confusingly arriving at the same place they already are.

---

### REQ-TASKS-UI-007: Pay the Full-List Cost Only When I Look

WHEN the Tasks panel is collapsed
THE SYSTEM SHALL NOT fetch the full task list, and SHALL instead fetch only the lightweight status counts the collapsed header summary needs (active / closed / blocked, and whether the current task is present)

WHEN I expand the panel for the first time
THE SYSTEM SHALL fetch the full list, with a brief "Loading..." indicator while the request is in flight

WHEN I navigate to a different conversation
THE SYSTEM SHALL clear the previously-displayed counts and task list (so the header summary doesn't lie about which conversation it represents), re-fetch the new conversation's counts immediately, and re-fetch its full list when next expanded

**Rationale:** The expensive read is the full list — it maps each task to the conversation working on it, which touches the conversation/project tables on top of the directory scan, and grows with task count. That stays deferred until the user expands the panel. The collapsed header still legitimately wants a count, so a dedicated count endpoint returns only the status tallies from a single directory scan, with no slug mapping — cheap enough to pay on mount. The "clear on conversation change" rule is the partner: navigating from conversation A (5 active tasks) to conversation B with the panel collapsed must not leave A's "5 active" lingering, so both the counts and the list are reset on `conversationId` change. The collapsed and expanded header read identically because both the count endpoint's tallies and the full list's reduction render through one shared label builder.
