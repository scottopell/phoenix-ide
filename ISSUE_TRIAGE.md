# Issue Triage

Tracked issues from QA agents. Each issue must be validated, then either have a failing test or a QA plan before fixing.

## Status Legend

- **Validated**: `pending` | `yes` | `no (test-artifact)` | `no (by-design)`
- **Unit Testable**: `yes` | `no` | `pending`
- **Failing Test**: `written` | `n/a` | `pending`
- **QA Plan**: `written` | `n/a` | `pending`

## Issues

### FTUX-01: Invalid path flash on page load

- **Source**: First-time UX agent
- **Severity**: confusing
- **Summary**: Settings panel briefly shows "invalid path" with red X before async validation completes. User sees error state flash for 1-2 seconds on every page load.
- **Validated**: yes
- **Unit Testable**: yes
- **Failing Test**: written
- **QA Plan**: pending
- **Validation notes**: Reproduced via Playwright. On load, StateBar shows "dir ... /Users/scott.opell" with "checking..." status for 2-3 seconds before settling to "dir [check] ~/". The initial state in NewConversationPage is `dirStatus: 'checking'` (line 27), which shows an ellipsis, not a red X. The flash is "checking..." not "invalid", but the user-facing confusion is real.
- **Test file**: `ui/src/pages/NewConversationPage.test.tsx`

### FTUX-02: No explanation of what Phoenix IDE is

- **Source**: First-time UX agent
- **Severity**: confusing
- **Summary**: Landing page has no onboarding, tagline, or explanation. New user with zero conversations sees a text input with no context about what the app does.
- **Validated**: yes
- **Unit Testable**: no
- **Failing Test**: n/a
- **QA Plan**: pending
- **Validation notes**: Confirmed via Playwright snapshot. Landing page shows only "New conversation" heading and a text input with placeholder "What would you like to work on?" -- no tagline, explanation, or onboarding.

### FTUX-03: Input retains stale text from previous sessions

- **Source**: First-time UX agent
- **Severity**: disorienting
- **Summary**: New conversation input shows pre-filled text from a previous session's draft. Draft persistence leaks across navigation contexts.
- **Validated**: no (test-artifact)
- **Unit Testable**: yes
- **Failing Test**: n/a
- **QA Plan**: n/a
- **Validation notes**: NewConversationPage uses `useState('')` for its draft (not localStorage-backed `useDraft`), so the new conversation input always starts empty. The conversation page's InputArea uses `useDraft(conversationId)` which IS localStorage-backed but keyed per conversation. The QA agent likely saw stale text in a conversation input, not the new conversation form. Not a cross-context leak.

### FTUX-04: After creating conversation, navigated to wrong one

- **Source**: First-time UX agent
- **Severity**: blocking
- **Summary**: User types message, clicks Send, and gets teleported to an unrelated existing conversation instead of the newly created one.
- **Validated**: no (cannot-reproduce)
- **Unit Testable**: yes
- **Failing Test**: pending
- **QA Plan**: pending
- **Validation notes**: Code inspection of NewConversationPage.tsx line 137 shows `navigate(/c/${conv.slug})` where `conv` is the API response from `api.createConversation()`. The navigation target comes directly from the create response. Could not reproduce via Playwright -- would need to create a real conversation which requires LLM calls. Possible race condition if two creates happen simultaneously, but the code path looks correct. Would need a specific reproduction scenario.

### FTUX-05: Project filter tabs unexplained and change unpredictably

- **Source**: First-time UX agent
- **Severity**: confusing
- **Summary**: Sidebar tabs use raw directory names (e.g., "phoenix-qa-j5") with no explanation. Active tab sometimes changes without user action when interacting with the settings panel.
- **Validated**: yes
- **Unit Testable**: no
- **Failing Test**: n/a
- **QA Plan**: pending
- **Validation notes**: Confirmed via Playwright snapshot -- tabs show raw directory basenames like "phoenix-qa-j5", "tmp.7dnAbLdFNa", "tmp.dgax4DAkwx" with no explanation. The "changes unpredictably" part could not be reproduced since settings panel interactions don't affect `activeProjectId` state. The raw directory names part is the real issue.

### FTUX-06: StateBar abbreviations are cryptic

- **Source**: First-time UX agent
- **Severity**: confusing
- **Summary**: StateBar shows "DIR [check] ~/ . MODEL claude-4-6 >" with no tooltips or explanation. The ">" chevron that opens settings is not discoverable.
- **Validated**: yes
- **Unit Testable**: no
- **Failing Test**: n/a
- **QA Plan**: pending
- **Validation notes**: Confirmed via Playwright snapshot. The StateBar button reads "dir [check] ~/ . model ... >" with no tooltip attribute. The chevron ">" toggles the settings panel but has no hover state or label indicating it is interactive.

### FTUX-07: Explore/Work/Standalone mode badges unexplained

- **Source**: First-time UX agent
- **Severity**: confusing
- **Summary**: Every conversation has a mode badge but the terms are never defined. No tooltip, no legend, no documentation visible in the UI.
- **Validated**: yes
- **Unit Testable**: no
- **Failing Test**: n/a
- **QA Plan**: pending
- **Validation notes**: Confirmed via Playwright snapshot. Every conversation shows an EXPLORE, WORK, or STANDALONE badge. The badge is a plain `<span>` with class `conv-mode-badge` -- no title attribute, no tooltip, no aria-label. No legend or explanation anywhere in the UI.

### FTUX-08: Conversation names are auto-generated slugs

- **Source**: First-time UX agent
- **Severity**: confusing
- **Summary**: Sidebar shows kebab-case slugs like "add-hello-file-task" instead of human-readable titles.
- **Validated**: yes
- **Unit Testable**: yes
- **Failing Test**: written
- **QA Plan**: pending
- **Validation notes**: Not a test artifact. The title_generator.rs generates a human-readable title, but `derive_slug()` in executor.rs converts it to kebab-case (e.g., "Add Hello File Task" becomes "add-hello-file-task"). ConversationList.tsx line 109 displays `conv.slug` -- there is no separate `title` field exposed to the UI. The slug IS the only display name.
- **Test file**: `src/db.rs` (test `test_ftux08_conversation_json_includes_title_field`)

### FTUX-09: File explorer shows entire home directory for Standalone

- **Source**: First-time UX agent
- **Severity**: confusing
- **Summary**: When CWD is ~/, file explorer shows all hidden files (.ssh, .aws, .docker, etc.). Overwhelming and exposes sensitive paths.
- **Validated**: yes
- **Unit Testable**: no
- **Failing Test**: n/a
- **QA Plan**: pending
- **Validation notes**: Confirmed via Playwright snapshot. Navigating to a Standalone conversation with CWD ~/ shows the file explorer with 60+ hidden directories (.ssh, .aws, .docker, .gnupg, .kube, etc.) and dozens of hidden files (.bash_history, .zsh_history, .claude.json, .google.env). Sensitive paths fully exposed.

### FTUX-10: Raw paths and model IDs in conversation header

- **Source**: First-time UX agent
- **Severity**: confusing
- **Summary**: Bottom bar shows full absolute path (/Users/scott.opell) and raw model ID (claude-sonnet-4-6) instead of human-friendly names.
- **Validated**: yes
- **Unit Testable**: no
- **Failing Test**: n/a
- **QA Plan**: pending
- **Validation notes**: Confirmed via Playwright snapshot. The conversation header banner shows "claude-sonnet-4-6" and "/Users/scott.opell" as raw strings. No tilde substitution for home directory, no friendly model display name.

### FTUX-11: Breadcrumb execution trail is unexplained

- **Source**: First-time UX agent
- **Severity**: confusing
- **Summary**: Trail shows "User -> bash -> LLM" with no explanation of what it means. "LLM" is jargon. Clickable segments have no affordance.
- **Validated**: yes
- **Unit Testable**: no
- **Failing Test**: n/a
- **QA Plan**: pending
- **Validation notes**: Confirmed via Playwright snapshot. Navigation bar shows "User -> bash -> LLM (retry 3)" with cursor=pointer on segments but no visual button affordance (no border, no background change). "LLM" is unexplained jargon.

### FTUX-12: "Background" button next to Send is unexplained

- **Source**: First-time UX agent
- **Severity**: confusing
- **Summary**: A "Background" button appears next to Send on the new conversation page with no tooltip or explanation of what it does.
- **Validated**: yes
- **Unit Testable**: no
- **Failing Test**: n/a
- **QA Plan**: pending
- **Validation notes**: Confirmed via Playwright snapshot. The "Background" button is visible next to "Send" on the new conversation form. It has a `title="Create and stay on this page"` in the code (line 238 of NewConversationPage.tsx), so there IS a tooltip -- but it only appears on hover. The button label alone is ambiguous.

### FTUX-13: Token counter meaning unclear

- **Source**: First-time UX agent
- **Severity**: confusing
- **Summary**: StateBar shows "4k / 200k tokens (2.2%)" with no explanation for non-technical users. Useful for power users but adds noise otherwise.
- **Validated**: yes
- **Unit Testable**: no
- **Failing Test**: n/a
- **QA Plan**: pending
- **Validation notes**: Confirmed via Playwright snapshot on conversation page. Banner shows "4k / 128k tokens (3.5%)" with a title attribute "4k / 128k tokens (3.5%)" -- tooltip just repeats the same text, no explanation of what tokens are or why the user should care.

### FTUX-14: System prompt visible by default

- **Source**: First-time UX agent
- **Severity**: confusing
- **Summary**: Every conversation shows a collapsed "SYSTEM PROMPT" header with preview text. Non-actionable for users who don't know what a system prompt is.
- **Validated**: yes
- **Unit Testable**: no
- **Failing Test**: n/a
- **QA Plan**: pending
- **Validation notes**: Confirmed via Playwright snapshot. Conversation page shows "System Prompt" with "show" toggle and preview text "You are a helpful AI assistant with access to tools for executing code, editing files...". Always visible at the top of every conversation.

### SIDE-01: Project tab clicks auto-submit pre-filled text

- **Source**: Multi-project sidebar agent
- **Severity**: blocking
- **Summary**: Clicking a project tab triggers Send if the input has pre-filled text, creating unintended conversations with real LLM calls. Costs money and pollutes conversation list.
- **Validated**: no (cannot-reproduce)
- **Unit Testable**: yes
- **Failing Test**: n/a
- **QA Plan**: pending
- **Validation notes**: Code inspection of Sidebar.tsx shows project tab clicks only call `setActiveProjectId(p.id)` (line 194) -- a pure state filter with no connection to message submission. NewConversationPage and SidebarNewForm handle sends independently. No code path connects tab clicks to send actions. The QA agent may have accidentally hit Enter while clicking, or this was a timing artifact.

### SIDE-02: "All" tab has no project labels

- **Source**: Multi-project sidebar agent
- **Severity**: disorienting
- **Summary**: In the "All" view, conversations show no project indicator. With 50+ conversations across 9 projects, you can't tell which project a conversation belongs to without clicking into it.
- **Validated**: yes
- **Unit Testable**: yes
- **Failing Test**: written
- **QA Plan**: pending
- **Validation notes**: Confirmed via Playwright snapshot. The "All" tab shows all 50+ conversations with only mode badges (EXPLORE/WORK/STANDALONE) -- no project name or path indicator. ConversationList.tsx renders `conv.slug` and `conv.conv_mode_label` but no project information.
- **Test file**: `ui/src/components/ConversationList.test.tsx` (test `SIDE-02`)

### SIDE-03: Escape in context menu navigates away

- **Source**: Multi-project sidebar agent
- **Severity**: disorienting
- **Summary**: Opening three-dot menu on a sidebar item and pressing Escape both closes the menu AND navigates to home route. Same class as the commit modal Escape bug (global keyboard nav handler).
- **Validated**: yes
- **Unit Testable**: yes
- **Failing Test**: written
- **QA Plan**: pending
- **Validation notes**: Reproduced via Playwright. Opened three-dot menu on a conversation while at /c/echo-hello-world-bash-command, pressed Escape -- navigated to / immediately. The context menu uses `e.stopPropagation()` only on click events (ConversationList.tsx line 59). The global Escape handler in useKeyboardNav.ts (line 20-31) listens on `window` keydown and navigates to / when on a /c/ path. The context menu does not intercept keyboard Escape events. Additionally, the context menu did NOT close on Escape -- it persisted (see SIDE-04).
- **Test file**: `ui/src/hooks/useKeyboardNav.test.tsx` (test `SIDE-03`)

### SIDE-04: Context menu persists across UI state changes

- **Source**: Multi-project sidebar agent
- **Severity**: confusing
- **Summary**: The Rename/Archive/Delete dropdown stays open when clicking project tabs, the "All" tab, or navigating to other views. Does not dismiss on click-away.
- **Validated**: yes
- **Unit Testable**: yes
- **Failing Test**: written
- **QA Plan**: pending
- **Validation notes**: Reproduced via Playwright. Opened context menu on conversation, then clicked away to another conversation -- menu stayed open. Context menu state (`expandedId`) in ConversationList.tsx is managed via `useState` and only toggles on explicit three-dot button clicks. No click-outside handler, no effect to close on navigation, no Escape key handler.
- **Test file**: `ui/src/components/ConversationList.test.tsx` (test `SIDE-04`)

### SIDE-05: Project tab overflow with no scroll indicator

- **Source**: Multi-project sidebar agent
- **Severity**: confusing
- **Summary**: Tab bar shows ~3 project tabs; the rest are hidden with no visual affordance (no arrows, fade, or "..." indicator).
- **Validated**: yes
- **Unit Testable**: no
- **Failing Test**: n/a
- **QA Plan**: pending
- **Validation notes**: Confirmed via screenshot. Tab bar shows "All", "phoenix-qa-j5", "phoenix-qa-j2", and a truncated "phoeni..." -- the remaining 7 tabs (phoenix-qa-test, python-data-scripts, tmp.*, go-microservice, phoenix-ide) are cut off with no scroll indicator, arrows, or overflow affordance. Accessibility tree shows all 10 tabs exist in DOM but are visually clipped.

### SIDE-06: "STANDALONE" badge truncates in sidebar

- **Source**: Multi-project sidebar agent
- **Severity**: confusing
- **Summary**: "Standalone" mode badge text truncates to "STANDALON..." in the conversation list. "Explore" and "Work" fit fine.
- **Validated**: yes
- **Unit Testable**: no
- **Failing Test**: n/a
- **QA Plan**: pending
- **Validation notes**: Confirmed via screenshot. The "STANDALONE" badge on "echo-hello-world-bash-command" is visibly truncated to "STANDALON..." while EXPLORE and WORK badges display fully.

### SIDE-07: No search or filter for conversations

- **Source**: Multi-project sidebar agent
- **Severity**: confusing
- **Summary**: With 50+ conversations, the only way to find one is linear scrolling. No search box or text filter.
- **Validated**: yes
- **Unit Testable**: n/a
- **Failing Test**: n/a
- **QA Plan**: n/a
- **Validation notes**: Confirmed -- no search input exists in the sidebar. This is a duplicate of existing task `tasks/0028-p4-ready--ui-search-filter.md`. No additional triage needed.

### SIDE-08: State dot color semantics unexplained

- **Source**: Multi-project sidebar agent
- **Severity**: confusing
- **Summary**: Green vs gray dots next to conversation names have no legend, tooltip, or documentation explaining what they mean.
- **Validated**: yes
- **Unit Testable**: no
- **Failing Test**: n/a
- **QA Plan**: pending
- **Validation notes**: Confirmed via Playwright snapshot and screenshot. Conversations show colored dots (green for active/idle, red for error) with class `conv-state-dot`. The dot element is a plain `<span>` with no title, tooltip, or aria-label. No legend exists anywhere in the UI.

### SIDE-09: Work conversations don't show task info in sidebar

- **Source**: Multi-project sidebar agent
- **Severity**: confusing
- **Summary**: The "Work" badge signals mode but not which task. Must click into conversation to see branch/task details. Duplicate of task 0606.
- **Validated**: yes
- **Unit Testable**: n/a
- **Failing Test**: n/a
- **QA Plan**: n/a
- **Validation notes**: Confirmed as duplicate. Task `tasks/0606-p2-ready--task-title-in-statebar.md` exists and covers this exact issue. No additional triage needed.
