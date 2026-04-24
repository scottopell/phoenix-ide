# Conversation Widget Inventory

All React components that render as a row or block in the conversation thread.
Source: frontend exploration 2026-04-24.

## Scope

**In scope:** anything rendered per-event in the conversation scroll — messages, tool
calls, tool results, streaming indicators, inline approval UI, inline error banners.

**Out of scope:** surrounding chrome — InputArea, Sidebar, command palette, settings
panels, file explorer, terminal panel, top-level toasts.

**Borderline — included with a flag:** `state-indicator-bar`, `jump-to-newest-button`.
These are chrome, not per-message, but they overlap conversation context enough to be
worth scoring. Strip them from the audit if you disagree.

## Widgets

### Core messages

| Slug | Location | What it renders |
| --- | --- | --- |
| user-message | `ui/src/components/MessageComponents.tsx:136–173` | User text + image attachments + “You” label + sent check |
| queued-user-message | `ui/src/components/MessageComponents.tsx:175–206` | Pending user message, hourglass + “Sending…” |
| agent-message | `ui/src/components/MessageComponents.tsx:218–349` | Phoenix agent text + embedded tool blocks |
| skill-invocation | `ui/src/components/MessageList.tsx:90–113` | `/skill_name` + args |
| system-message | `ui/src/components/MessageList.tsx:124–133` | System plain text, no sender |

### Tool call renderers

All dispatch through `formatToolInput` at `MessageComponents.tsx:87–129`; shared output
section at `377–534`.

| Slug | Location | Dedicated input formatter? |
| --- | --- | --- |
| tool-call-bash | `MessageComponents.tsx:87–94` | Yes — `$command` with multiline flag |
| tool-call-think | `MessageComponents.tsx:95–97` | Yes — cleaned thought text |
| tool-call-patch | `MessageComponents.tsx:99–105` + `PatchFileSummary.tsx:83–113` | Yes — file/op summary + diff file list |
| tool-call-keyword-search | `MessageComponents.tsx:107–111` | Yes — query + terms |
| tool-call-read-image | `MessageComponents.tsx:113–115` | Yes — path |
| tool-call-spawn-agents | `MessageComponents.tsx:117–123` | Yes — parallel task count |
| tool-call-generic | `MessageComponents.tsx:125–129` | **No — catch-all for unspecialized tools** |

**Generic catch-all covers:** `browser_*`, `mcp`, `read_file`, `search`,
`ask_user_question`, `propose_task`, `terminal_command_history`,
`terminal_last_command`, `skill`. All render as raw JSON input — audit-relevant because
this is where low-quality rendering hides.
One eval file covers the widget; note in that file which tools fall through.

**Live/history pairings (reference only):** Two tools in the generic catch-all have a
live-interaction modal counterpart.
Both tools are low-frequency, so the pairing is noted for completeness but should not
dominate the audit.

| Tool | Live modal | History view |
| --- | --- | --- |
| `ask_user_question` | `question-panel` | `tool-call-generic` |
| `propose_task` | `task-approval-reader` | `tool-call-generic` |

### Tool result renderers

| Slug | Location | What it renders |
| --- | --- | --- |
| tool-result-image | `MessageComponents.tsx:401–416, 463–471` | Base64 image from display_data or parsed JSON |
| tool-result-subagent-summary | `MessageComponents.tsx:558–612` | Per-agent outcomes with expand |
| tool-result-text | `MessageComponents.tsx:419–514` | Plain text output; inline if <200 chars, collapsible with 3-line faded preview if longer |

(Short and long text result variants consolidated into one eval — the threshold split
itself is worth evaluating.)

### Streaming / progress

| Slug | Location | What it renders |
| --- | --- | --- |
| streaming-message | `StreamingMessage.tsx:31–104` | Live token stream with markdown + cursor |
| subagent-status-block | `MessageComponents.tsx:663–694` | Live progress: completed/total + pending spinners |

### Inline approval UI

| Slug | Location | What it renders |
| --- | --- | --- |
| task-approval-reader | `TaskApprovalReader.tsx:179–582` | Modal plan with annotation + Discard/Feedback/Approve |
| question-panel | `QuestionPanel.tsx:44–1000` | Multi-step question wizard |

### Inline / chrome (borderline)

| Slug | Location | What it renders |
| --- | --- | --- |
| breadcrumb-bar | `BreadcrumbBar.tsx:43–154` | Conversation progress trail |
| state-indicator-bar | `StateBar.tsx:79–510` | Top status bar + context window |
| jump-to-newest-button | `MessageList.tsx:338–344` | “↓ New messages” floating button |

## Summary

**22 widget evals** (short/long text result consolidated).

Audit-relevant clusters:

- **Tool call renderers** share dispatch but vary in dedicated input formatting →
  consistency is the pivotal dimension for this group.
- **Generic fallback** (`tool-call-generic`) covers 9+ tools with no specialization →
  almost certainly a bottom scorer, and the most-users-helped-per-unit-of-fix target.
- **Inline approval UI** (`task-approval-reader`, `question-panel`) are
  multi-hundred-line components → likely score high on density but poorly on
  scannability.
