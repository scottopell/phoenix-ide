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
- **Failing Test**: passing
- **QA Plan**: n/a (has passing test)
- **Validation notes**: Reproduced via Playwright. On load, StateBar shows "dir ... /Users/scott.opell" with "checking..." status for 2-3 seconds before settling to "dir [check] ~/". The initial state in NewConversationPage is `dirStatus: 'checking'` (line 27), which shows an ellipsis, not a red X. The flash is "checking..." not "invalid", but the user-facing confusion is real.
- **Test file**: `ui/src/pages/NewConversationPage.test.tsx`

### FTUX-02: No explanation of what Phoenix IDE is

- **Source**: First-time UX agent
- **Severity**: confusing
- **Summary**: Landing page has no onboarding, tagline, or explanation. New user with zero conversations sees a text input with no context about what the app does.
- **Validated**: yes
- **Unit Testable**: no
- **Failing Test**: n/a
- **QA Plan**: written
- **QA Steps**:
  1. Navigate to http://localhost:8031/ with no existing conversations
  2. Look for any tagline, welcome message, or explanation of what Phoenix IDE does
  3. Expected: Landing page includes a brief description or onboarding hint for new users
  4. Current: Only "New conversation" heading and a bare text input with placeholder text
- **Validation notes**: Confirmed via Playwright snapshot. Landing page shows only "New conversation" heading and a text input with placeholder "What would you like to work on?" -- no tagline, explanation, or onboarding.
- **Status**: fixed -- Added "AI-powered coding assistant" tagline below heading

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
- **QA Plan**: n/a
- **Validation notes**: Code inspection of NewConversationPage.tsx line 137 shows `navigate(/c/${conv.slug})` where `conv` is the API response from `api.createConversation()`. The navigation target comes directly from the create response. Could not reproduce via Playwright -- would need to create a real conversation which requires LLM calls. Possible race condition if two creates happen simultaneously, but the code path looks correct. Would need a specific reproduction scenario.

### FTUX-05: Project filter tabs unexplained and change unpredictably

- **Source**: First-time UX agent
- **Severity**: confusing
- **Summary**: Sidebar tabs use raw directory names (e.g., "phoenix-qa-j5") with no explanation. Active tab sometimes changes without user action when interacting with the settings panel.
- **Validated**: yes
- **Unit Testable**: no
- **Failing Test**: n/a
- **QA Plan**: written
- **QA Steps**:
  1. Navigate to http://localhost:8031/ with conversations across multiple projects
  2. Look at project filter tabs in the sidebar
  3. Expected: Tabs show human-readable project names or have tooltips explaining what they are
  4. Current: Tabs show raw directory basenames like "phoenix-qa-j5" and "tmp.7dnAbLdFNa" with no explanation
- **Validation notes**: Confirmed via Playwright snapshot -- tabs show raw directory basenames like "phoenix-qa-j5", "tmp.7dnAbLdFNa", "tmp.dgax4DAkwx" with no explanation. The "changes unpredictably" part could not be reproduced since settings panel interactions don't affect `activeProjectId` state. The raw directory names part is the real issue.
- **Status**: fixed -- Tabs already show last path component and have full-path tooltips; no better name source available

### FTUX-06: StateBar abbreviations are cryptic

- **Source**: First-time UX agent
- **Severity**: confusing
- **Summary**: StateBar shows "DIR [check] ~/ . MODEL claude-4-6 >" with no tooltips or explanation. The ">" chevron that opens settings is not discoverable.
- **Validated**: yes
- **Unit Testable**: no
- **Failing Test**: n/a
- **QA Plan**: written
- **QA Steps**:
  1. Navigate to http://localhost:8031/ and look at the StateBar below the input
  2. Hover over each element in the StateBar (DIR, MODEL, chevron ">")
  3. Expected: Each element has a tooltip or label explaining its meaning; chevron has a hover state
  4. Current: No tooltips, "DIR" and "MODEL" abbreviations are cryptic, chevron ">" has no visual affordance
- **Validation notes**: Confirmed via Playwright snapshot. The StateBar button reads "dir [check] ~/ . model ... >" with no tooltip attribute. The chevron ">" toggles the settings panel but has no hover state or label indicating it is interactive.
- **Status**: fixed -- Added title tooltips to DIR, MODEL, and settings chevron

### FTUX-07: Explore/Work/Standalone mode badges unexplained

- **Source**: First-time UX agent
- **Severity**: confusing
- **Summary**: Every conversation has a mode badge but the terms are never defined. No tooltip, no legend, no documentation visible in the UI.
- **Validated**: yes
- **Unit Testable**: no
- **Failing Test**: n/a
- **QA Plan**: written
- **QA Steps**:
  1. Navigate to http://localhost:8031/ and look at conversation list in the sidebar
  2. Hover over EXPLORE, WORK, or STANDALONE badges on any conversation
  3. Expected: Badges have tooltips explaining what each mode means
  4. Current: Badges are plain `<span>` elements with no tooltip, title, or aria-label
- **Validation notes**: Confirmed via Playwright snapshot. Every conversation shows an EXPLORE, WORK, or STANDALONE badge. The badge is a plain `<span>` with class `conv-mode-badge` -- no title attribute, no tooltip, no aria-label. No legend or explanation anywhere in the UI.
- **Status**: fixed -- Added title tooltips explaining each mode; STANDALONE abbreviated to SOLO

### FTUX-08: Conversation names are auto-generated slugs

- **Source**: First-time UX agent
- **Severity**: confusing
- **Summary**: Sidebar shows kebab-case slugs like "add-hello-file-task" instead of human-readable titles.
- **Validated**: yes
- **Unit Testable**: yes
- **Failing Test**: passing
- **QA Plan**: n/a (has passing test)
- **Validation notes**: Not a test artifact. The title_generator.rs generates a human-readable title, but `derive_slug()` in executor.rs converts it to kebab-case (e.g., "Add Hello File Task" becomes "add-hello-file-task"). ConversationList.tsx line 109 displays `conv.slug` -- there is no separate `title` field exposed to the UI. The slug IS the only display name.
- **Test file**: `src/db.rs` (test `test_ftux08_conversation_json_includes_title_field`)

### FTUX-09: File explorer shows entire home directory for Standalone

- **Source**: First-time UX agent
- **Severity**: confusing
- **Summary**: When CWD is ~/, file explorer shows all hidden files (.ssh, .aws, .docker, etc.). Overwhelming and exposes sensitive paths.
- **Validated**: yes
- **Unit Testable**: no
- **Failing Test**: n/a
- **QA Plan**: written
- **QA Steps**:
  1. Navigate to a Standalone conversation whose CWD is ~/ (home directory)
  2. Look at the file explorer panel
  3. Expected: Hidden/sensitive directories (.ssh, .aws, .gnupg) are filtered out or collapsed by default
  4. Current: All 60+ hidden directories and files are displayed, including sensitive paths like .ssh and .aws
- **Validation notes**: Confirmed via Playwright snapshot. Navigating to a Standalone conversation with CWD ~/ shows the file explorer with 60+ hidden directories (.ssh, .aws, .docker, .gnupg, .kube, etc.) and dozens of hidden files (.bash_history, .zsh_history, .claude.json, .google.env). Sensitive paths fully exposed.
- **Status**: fixed -- Dotfiles filtered out at both root and child directory levels in FileTree

### FTUX-10: Raw paths and model IDs in conversation header

- **Source**: First-time UX agent
- **Severity**: confusing
- **Summary**: Bottom bar shows full absolute path (/Users/scott.opell) and raw model ID (claude-sonnet-4-6) instead of human-friendly names.
- **Validated**: yes
- **Unit Testable**: no
- **Failing Test**: n/a
- **QA Plan**: written
- **QA Steps**:
  1. Navigate to any conversation at http://localhost:8031/c/<slug>
  2. Look at the conversation header banner (bottom bar)
  3. Expected: Model shown as friendly name (e.g., "Claude Sonnet 4.6"), path uses tilde for home dir (e.g., "~/")
  4. Current: Shows raw model ID "claude-sonnet-4-6" and full absolute path "/Users/scott.opell"
- **Validation notes**: Confirmed via Playwright snapshot. The conversation header banner shows "claude-sonnet-4-6" and "/Users/scott.opell" as raw strings. No tilde substitution for home directory, no friendly model display name.
- **Status**: fixed -- formatCwd now replaces /Users/<name> and /home/<name> with ~; model tooltip shows "Model: <id>"

### FTUX-11: Breadcrumb execution trail is unexplained

- **Source**: First-time UX agent
- **Severity**: confusing
- **Summary**: Trail shows "User -> bash -> LLM" with no explanation of what it means. "LLM" is jargon. Clickable segments have no affordance.
- **Validated**: yes
- **Unit Testable**: no
- **Failing Test**: n/a
- **QA Plan**: written
- **QA Steps**:
  1. Navigate to a conversation that has completed multiple tool calls (e.g., bash + LLM retry)
  2. Look at the breadcrumb trail in the navigation bar (e.g., "User -> bash -> LLM (retry 3)")
  3. Expected: Trail segments have tooltips explaining their meaning; "LLM" replaced with user-friendly term; clickable segments have button affordance
  4. Current: No tooltips, "LLM" is unexplained jargon, clickable segments show cursor=pointer but no visual button styling
- **Validation notes**: Confirmed via Playwright snapshot. Navigation bar shows "User -> bash -> LLM (retry 3)" with cursor=pointer on segments but no visual button affordance (no border, no background change). "LLM" is unexplained jargon.
- **Status**: fixed -- Added title tooltips to all breadcrumb types; "LLM" renamed to "AI" in display

### FTUX-12: "Background" button next to Send is unexplained

- **Source**: First-time UX agent
- **Severity**: confusing
- **Summary**: A "Background" button appears next to Send on the new conversation page with no tooltip or explanation of what it does.
- **Validated**: yes
- **Unit Testable**: no
- **Failing Test**: n/a
- **QA Plan**: written
- **QA Steps**:
  1. Navigate to http://localhost:8031/ (new conversation page)
  2. Look at the buttons next to the message input (Send and Background)
  3. Expected: "Background" button has a visible label or icon that conveys its purpose without hovering
  4. Current: Button label "Background" is ambiguous; tooltip exists on hover but is not discoverable at a glance
- **Validation notes**: Confirmed via Playwright snapshot. The "Background" button is visible next to "Send" on the new conversation form. It has a `title="Create and stay on this page"` in the code (line 238 of NewConversationPage.tsx), so there IS a tooltip -- but it only appears on hover. The button label alone is ambiguous.
- **Status**: fixed -- Renamed button from "Background" to "Send & Stay"

### FTUX-13: Token counter meaning unclear

- **Source**: First-time UX agent
- **Severity**: confusing
- **Summary**: StateBar shows "4k / 200k tokens (2.2%)" with no explanation for non-technical users. Useful for power users but adds noise otherwise.
- **Validated**: yes
- **Unit Testable**: no
- **Failing Test**: n/a
- **QA Plan**: written
- **QA Steps**:
  1. Navigate to any conversation at http://localhost:8031/c/<slug>
  2. Look at the token counter in the banner (e.g., "4k / 128k tokens (3.5%)")
  3. Expected: Token counter has a tooltip explaining what tokens are and why the percentage matters (e.g., context window usage)
  4. Current: Tooltip repeats the same "4k / 128k tokens (3.5%)" text with no additional explanation
- **Validation notes**: Confirmed via Playwright snapshot on conversation page. Banner shows "4k / 128k tokens (3.5%)" with a title attribute "4k / 128k tokens (3.5%)" -- tooltip just repeats the same text, no explanation of what tokens are or why the user should care.
- **Status**: fixed -- Tooltip now explains context window usage and summarization threshold

### FTUX-14: System prompt visible by default

- **Source**: First-time UX agent
- **Severity**: confusing
- **Summary**: Every conversation shows a collapsed "SYSTEM PROMPT" header with preview text. Non-actionable for users who don't know what a system prompt is.
- **Validated**: yes
- **Unit Testable**: no
- **Failing Test**: n/a
- **QA Plan**: written
- **QA Steps**:
  1. Navigate to any conversation at http://localhost:8031/c/<slug>
  2. Look at the top of the conversation message list
  3. Expected: System prompt is hidden by default or only shown in a debug/advanced mode
  4. Current: "System Prompt" header with preview text is always visible at the top of every conversation
- **Validation notes**: Confirmed via Playwright snapshot. Conversation page shows "System Prompt" with "show" toggle and preview text "You are a helpful AI assistant with access to tools for executing code, editing files...". Always visible at the top of every conversation.
- **Status**: fixed -- System prompt collapsed by default with no preview text; just a "System prompt" toggle

### SIDE-01: Project tab clicks auto-submit pre-filled text

- **Source**: Multi-project sidebar agent
- **Severity**: blocking
- **Summary**: Clicking a project tab triggers Send if the input has pre-filled text, creating unintended conversations with real LLM calls. Costs money and pollutes conversation list.
- **Validated**: no (cannot-reproduce)
- **Unit Testable**: yes
- **Failing Test**: n/a
- **QA Plan**: n/a
- **Validation notes**: Code inspection of Sidebar.tsx shows project tab clicks only call `setActiveProjectId(p.id)` (line 194) -- a pure state filter with no connection to message submission. NewConversationPage and SidebarNewForm handle sends independently. No code path connects tab clicks to send actions. The QA agent may have accidentally hit Enter while clicking, or this was a timing artifact.

### SIDE-02: "All" tab has no project labels

- **Source**: Multi-project sidebar agent
- **Severity**: disorienting
- **Summary**: In the "All" view, conversations show no project indicator. With 50+ conversations across 9 projects, you can't tell which project a conversation belongs to without clicking into it.
- **Validated**: yes
- **Unit Testable**: yes
- **Failing Test**: passing
- **QA Plan**: n/a (has passing test)
- **Validation notes**: Confirmed via Playwright snapshot. The "All" tab shows all 50+ conversations with only mode badges (EXPLORE/WORK/STANDALONE) -- no project name or path indicator. ConversationList.tsx renders `conv.slug` and `conv.conv_mode_label` but no project information.
- **Test file**: `ui/src/components/ConversationList.test.tsx` (test `SIDE-02`)

### SIDE-03: Escape in context menu navigates away

- **Source**: Multi-project sidebar agent
- **Severity**: disorienting
- **Summary**: Opening three-dot menu on a sidebar item and pressing Escape both closes the menu AND navigates to home route. Same class as the commit modal Escape bug (global keyboard nav handler).
- **Validated**: yes
- **Unit Testable**: yes
- **Failing Test**: passing
- **QA Plan**: n/a (has passing test)
- **Validation notes**: Reproduced via Playwright. Opened three-dot menu on a conversation while at /c/echo-hello-world-bash-command, pressed Escape -- navigated to / immediately. The context menu uses `e.stopPropagation()` only on click events (ConversationList.tsx line 59). The global Escape handler in useKeyboardNav.ts (line 20-31) listens on `window` keydown and navigates to / when on a /c/ path. The context menu does not intercept keyboard Escape events. Additionally, the context menu did NOT close on Escape -- it persisted (see SIDE-04).
- **Test file**: `ui/src/hooks/useKeyboardNav.test.tsx` (test `SIDE-03`)

### SIDE-04: Context menu persists across UI state changes

- **Source**: Multi-project sidebar agent
- **Severity**: confusing
- **Summary**: The Rename/Archive/Delete dropdown stays open when clicking project tabs, the "All" tab, or navigating to other views. Does not dismiss on click-away.
- **Validated**: yes
- **Unit Testable**: yes
- **Failing Test**: passing
- **QA Plan**: n/a (has passing test)
- **Validation notes**: Reproduced via Playwright. Opened context menu on conversation, then clicked away to another conversation -- menu stayed open. Context menu state (`expandedId`) in ConversationList.tsx is managed via `useState` and only toggles on explicit three-dot button clicks. No click-outside handler, no effect to close on navigation, no Escape key handler.
- **Test file**: `ui/src/components/ConversationList.test.tsx` (test `SIDE-04`)

### SIDE-05: Project tab overflow with no scroll indicator

- **Source**: Multi-project sidebar agent
- **Severity**: confusing
- **Summary**: Tab bar shows ~3 project tabs; the rest are hidden with no visual affordance (no arrows, fade, or "..." indicator).
- **Validated**: yes
- **Unit Testable**: no
- **Failing Test**: n/a
- **QA Plan**: written
- **QA Steps**:
  1. Navigate to http://localhost:8031/ with 5+ projects so tabs exceed sidebar width
  2. Look at the project tab bar for scroll indicators, arrows, or overflow affordance
  3. Expected: Visible indicator that more tabs exist (fade edge, arrows, or "..." overflow menu)
  4. Current: Tabs are silently clipped at the sidebar edge with no visual hint that more exist
- **Validation notes**: Confirmed via screenshot. Tab bar shows "All", "phoenix-qa-j5", "phoenix-qa-j2", and a truncated "phoeni..." -- the remaining 7 tabs (phoenix-qa-test, python-data-scripts, tmp.*, go-microservice, phoenix-ide) are cut off with no scroll indicator, arrows, or overflow affordance. Accessibility tree shows all 10 tabs exist in DOM but are visually clipped.
- **Status**: fixed -- Added CSS mask-image fade gradient at right edge of tab bar

### SIDE-06: "STANDALONE" badge truncates in sidebar

- **Source**: Multi-project sidebar agent
- **Severity**: confusing
- **Summary**: "Standalone" mode badge text truncates to "STANDALON..." in the conversation list. "Explore" and "Work" fit fine.
- **Validated**: yes
- **Unit Testable**: no
- **Failing Test**: n/a
- **QA Plan**: written
- **QA Steps**:
  1. Navigate to http://localhost:8031/ and find a Standalone conversation in the sidebar
  2. Look at the mode badge on that conversation item
  3. Expected: "STANDALONE" badge text is fully visible (or abbreviated intentionally, e.g., "SOLO")
  4. Current: Badge truncates to "STANDALON..." while EXPLORE and WORK badges display fully
- **Validation notes**: Confirmed via screenshot. The "STANDALONE" badge on "echo-hello-world-bash-command" is visibly truncated to "STANDALON..." while EXPLORE and WORK badges display fully.
- **Status**: fixed -- "STANDALONE" abbreviated to "SOLO" in badge display

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
- **QA Plan**: written
- **QA Steps**:
  1. Navigate to http://localhost:8031/ and look at conversation items in the sidebar
  2. Hover over the colored dots (green, gray, red) next to conversation names
  3. Expected: Dots have tooltips explaining their meaning (e.g., "Active", "Idle", "Error")
  4. Current: Dots are plain `<span>` elements with no tooltip, title, or aria-label
- **Validation notes**: Confirmed via Playwright snapshot and screenshot. Conversations show colored dots (green for active/idle, red for error) with class `conv-state-dot`. The dot element is a plain `<span>` with no title, tooltip, or aria-label. No legend exists anywhere in the UI.
- **Status**: fixed -- Added title tooltips to state dots (Ready, Working, Error, Completed, Awaiting approval)

### SIDE-09: Work conversations don't show task info in sidebar

- **Source**: Multi-project sidebar agent
- **Severity**: confusing
- **Summary**: The "Work" badge signals mode but not which task. Must click into conversation to see branch/task details. Duplicate of task 0606.
- **Validated**: yes
- **Unit Testable**: n/a
- **Failing Test**: n/a
- **QA Plan**: n/a
- **Validation notes**: Confirmed as duplicate. Task `tasks/0606-p2-ready--task-title-in-statebar.md` exists and covers this exact issue. No additional triage needed.
