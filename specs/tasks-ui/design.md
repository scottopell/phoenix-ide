# Tasks UI Design

This document describes the technical architecture for the Tasks panel
and Task viewer, implementing `specs/tasks-ui/requirements.md`.

## Component Boundary

```
┌─ Conversation page (specs/conversation-ui/) ────────────────────┐
│                                                                │
│   ┌─ TasksPanel.tsx (this spec) ───────────────────────────┐  │
│   │  collapsed header → "Tasks · 3 active · 7 closed"      │  │
│   │  expanded → groups by status                           │  │
│   │    in-progress / ready / blocked   (open by default)   │  │
│   │    brainstorming                                        │  │
│   │    done / wont-do                  (closed by default) │  │
│   │                                                         │  │
│   │  click row → onTaskClick(task)                          │  │
│   └─────────────────────────────────────────────────────────┘  │
│                                                                │
│   ┌─ TaskViewer.tsx (this spec) ──────────────────────────┐   │
│   │  details: status / priority / slug / filename         │   │
│   │  conversation link, if any                            │   │
│   │  full body with frontmatter stripped                  │   │
│   │                                                       │   │
│   │  "Start working" → seeded conversation                │   │
│   │      (specs/seeded-conversations/)                    │   │
│   └───────────────────────────────────────────────────────┘   │
│                                                                │
└────────────────────────────────────────────────────────────────┘
                             ↕ HTTP
┌─ Backend ──────────────────────────────────────────────────────┐
│  GET /api/conversations/:id/tasks  → TaskEntry[]              │
│  GET /api/files/read?path=...      → file contents            │
│  POST /api/conversations           → seeded conversation      │
└────────────────────────────────────────────────────────────────┘
```

## API Surface

The panel reads three endpoints:

- `api.listConversationTasks(conversationId)` — returns `{ tasks: TaskEntry[] }` where `TaskEntry` carries `{ id, priority, status, slug, path?, conversation_slug? }`. The backend resolves `path` server-side from `id-priority-status--slug.md` so the viewer can fetch by absolute path without reconstructing it.
- `GET /api/files/read?path=<absolute>` — generic read endpoint used by `TaskViewer` for the body. Same endpoint the prose reader uses.
- `api.createConversation(...)` — used by the "Start working" path. Owned by `specs/seeded-conversations/`.

## State (`TasksPanel`)

| State | Purpose | Driven by |
|---|---|---|
| `expanded` (boolean) | Whether the panel body is visible | Header click |
| `tasks` (TaskEntry[]) | The current conversation's task list | `useEffect` on `[conversationId, expanded]` — fetches only when expanded |
| `loading` (boolean) | Brief spinner during fetch | The same effect |
| `groupExpanded` (Record<status, boolean>) | Per-status collapse state | Group header click; defaults: active=true, terminal=false |

## State (`TaskViewer`)

| State | Purpose |
|---|---|
| `content` / `rawContent` (string?) | Rendered body (frontmatter stripped) and raw markdown (used for the seed prompt) |
| `loading` / `error` | Standard fetch lifecycle |
| `seeding` / `seedError` | "Start working" in-flight + failure surface |

`rawContent` and `content` are split because the seed-prompt builder
wants the full file (frontmatter included) — the agent should see the
same view a developer would see when opening the file — but the
on-screen Content section deliberately strips it.

## Status / Priority Visual Encoding

The panel uses CSS classes derived from the status and priority enums:

- Status: `tasks-status-<status>` for the colour dot beside each group (e.g. `tasks-status-in-progress`, `tasks-status-blocked`).
- Priority: one of `tasks-pri-p0` through `tasks-pri-p4` on each row's badge. Unknown priorities fall back to `tasks-pri-p3` (sensible middle).
- Terminal status flag: `TERMINAL_STATUSES = new Set(['done', 'wont-do'])` is the single source of truth for "this task is closed" used by the count summary, the default group-collapsed state, and the row's dim styling.

## "Start working" Seed Prompt

`buildTaskPrompt(task, body)` (`TaskViewer.tsx:202-215`) constructs the
prompt that the user reviews before sending in the new conversation:

```
I want to work on task <id>.

Here's the task:

---
<full task markdown including frontmatter>
---

Please read the scope carefully and start executing. Ask before doing
anything destructive (commits, force pushes, deleting files outside
the task's stated scope). If the scope is unclear in any way, stop
and ask for clarification before writing code.
```

The frontmatter is intentionally included: it carries `status`,
`priority`, and `artifact` which the agent might need to honour. The
on-screen viewer strips frontmatter for the user's reading
convenience; the seed prompt does not.

The seeded conversation is created with:

- `cwd = parentConversation.cwd` (same working directory as the parent)
- `seed_parent_id = parentConversation.id` (parent breadcrumb on the new conversation)
- `seed_label = "Work on task <id>: <slug>"` (rendered alongside the breadcrumb per `specs/seeded-conversations/` REQ-SEED-004)
- empty initial text — the prompt lives in `localStorage` keyed by `seed-draft:<newConv.id>` (REQ-SEED-001), hydrated by the new conversation page on mount

After successful creation the user is navigated to `/c/<newConv.slug>`.

## Defer-Until-Expanded Fetch Pattern

The list fetch is gated by `expanded` in the effect's dependency
array. This is the correct cost model: tasks are reference data, not
something the user always needs visible. A user who never opens the
panel never pays for the fetch, even across many conversation mounts.

The trade-off is one extra round-trip the first time the user
expands. Acceptable: that cost is paid once per conversation per
panel-open, never on the conversation's hot mount path.

## Cross-Spec Hand-Offs

- **Seed conversation creation** (`TaskViewer.tsx:79-112`) — the entire flow (cwd, seed_parent_id, seed_label, draft key) follows `specs/seeded-conversations/` REQ-SEED-001 through -004.
- **Conversation navigation** (`TasksPanel.tsx:159`, `TaskViewer.tsx:167`) — uses `useNavigate` against the `/c/<slug>` route owned by `specs/conversation-ui/`.
- **File read** (`TaskViewer.tsx:49`) — the `/api/files/read` endpoint is shared with the prose reader (`specs/conversation-ui/` indirectly via the file viewer).

## Open Questions

None. The UI is intentionally read-mostly; the only mutation it can
trigger is the seeded sub-conversation, which is owned by another
spec. The two latent gaps in similar UIs (assist-setup error
visibility in terminal-panel; conflict UX) don't apply here — the
seed-error path already surfaces failures inline (`TaskViewer.tsx:35,108-111,136-138`).
