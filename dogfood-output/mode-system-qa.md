# Mode System QA Report

Date: 2026-04-03
Viewport: 1440x900 (desktop)
App URL: http://localhost:8042 (API: http://localhost:8033)

## Test A: New Conversation Page

**Screenshot:** `screenshots/mode-qa-A-new-conv.png`, `screenshots/mode-qa-A-git-subtitle.png`

| Verification | Result | Notes |
|---|---|---|
| Directory picker shows current dir | PASS | Shows `~/` with green checkmark |
| Non-git dir subtitle shows "Direct mode -- full tool access" | PASS | Confirmed via DOM query and page text |
| Git repo subtitle shows "Git project -- starts in Explore mode (read-only)" | PASS | Changed dir to `/Users/scott.opell/dev/phoenix-ide`, subtitle updated correctly |
| Subtitle element class | PASS | Uses `new-conv-mode-preview` class |

## Test B: Explore Conversation

**Screenshot:** `screenshots/mode-qa-B-explore-conv.png`

Tested: `network-connectivity-test` (Explore, created 2026-04-03)

| Verification | Result | Notes |
|---|---|---|
| StateBar shows blue "Explore (read-only)" pill | PASS | Text: "Explore (read-only)", class: `statebar-mode--explore`, color: rgb(88, 166, 255) (blue), bg: srgb blue at 15% opacity |
| InputArea placeholder says "Explore this codebase or describe a change to plan..." | PASS | Exact match confirmed via textarea.placeholder |
| Sidebar badge says "Explore" | PASS | Badge text: "Explore", class: `conv-mode-badge` |
| State indicator shows "ready" | PASS | State text: "ready" (idle conversation) |

## Test C: Standalone/Direct Conversation

**Screenshot:** `screenshots/mode-qa-C-direct-conv.png`

Tested: `say-hi-three-words` (Direct/Standalone)

| Verification | Result | Notes |
|---|---|---|
| StateBar shows muted "Direct" pill (not "SOLO") | PASS | Text: "Direct", class: `statebar-mode--direct`, color: rgb(110, 118, 129) (muted gray) |
| Sidebar badge says "Direct" (not "SOLO") | PASS | Badge text: "Direct" |
| No "SOLO" text anywhere on page | PASS | Full body text search confirms zero occurrences |
| InputArea placeholder says "Type a message..." | PASS | Standard placeholder for non-Explore modes |
| No "SOLO" in UI source code | PASS | Grep of `ui/src/` found zero matches for SOLO/Solo |

## Test D: Work Conversation

**Screenshot:** `screenshots/mode-qa-D-work-conv.png`

Tested: `log-parsing-performance-review` (Work)

| Verification | Result | Notes |
|---|---|---|
| StateBar shows green "Work" pill | PASS | Text: "Work", class: `statebar-mode--work`, color: rgb(63, 185, 80) (green), bg: srgb green at 15% opacity |
| Sidebar badge says "Work" | PASS | Badge text: "Work" |
| State indicator shows "ready" | PASS | Idle Work conversation |

## Test E: Terminal Conversation

**Screenshot:** `screenshots/mode-qa-E-terminal-bottom.png`, `screenshots/mode-qa-E-terminal-sysprompt.png`

Tested: `add-greeting-module-rust` (Terminal/Explore), `add-farewell-module-plan` (Terminal/Explore)

| Verification | Result | Notes |
|---|---|---|
| StateBar dot is muted (not green) with text "completed" | PASS | Dot class: `dot terminal`, bg: rgb(110, 118, 129) (muted gray). State text: "completed", color: rgb(139, 148, 158) |
| Terminal banner shows system message context | PASS | Banner text: "Task completed. Squash merged to main as 1beebe7." with `terminal-banner-context` class |
| "Start new conversation" button present | PASS | Button visible below the context message |
| InputArea is NOT shown | PASS | No textarea or input area found in DOM |
| Sidebar dot for terminal conversations is gray | PASS | Class `conv-state-dot terminal`, bg: rgb(110, 118, 129) -- distinct from green idle dots |
| Second terminal conversation also shows "completed" | PASS | `add-farewell-module-plan` also shows "completed" state |

## Test F: propose_task Rename

| Verification | Result | Notes |
|---|---|---|
| Source code uses "propose_task" in system prompt | PASS | `src/system_prompt.rs:444` reads "use the propose_task tool" |
| Source code uses "propose_task" throughout | PASS | All references in `src/` use `propose_task` (state.rs, transition.rs, tools.rs, propose_task.rs, handlers.rs) |
| No "propose_plan" in source code | PASS | Zero matches for `propose_plan` in `src/` |
| Old conversations still show "propose_plan" | N/A | Expected -- historical system prompts were generated before rename. Not a bug. |

## Test G: Console Errors

| Verification | Result | Notes |
|---|---|---|
| No JS errors after navigating multiple conversations | PASS | Console error interceptors captured zero errors across 5+ conversation navigations |

## Sidebar Badge Summary

Badge distribution across all conversations:
- **Direct**: 18 conversations
- **Explore**: 58 conversations  
- **Work**: 1 conversation

All badges use class `conv-mode-badge` with consistent muted gray styling in the sidebar. No "SOLO" badges found.

## Color Scheme Summary

| Mode | StateBar Pill Color | Background |
|---|---|---|
| Explore (read-only) | rgb(88, 166, 255) -- blue | srgb blue at 15% opacity |
| Work | rgb(63, 185, 80) -- green | srgb green at 15% opacity |
| Direct | rgb(110, 118, 129) -- muted gray | rgb(33, 38, 45) -- dark |

| State | Dot Color | Text |
|---|---|---|
| Idle (ready) | rgb(63, 185, 80) -- green | "ready" |
| Terminal (completed) | rgb(110, 118, 129) -- muted gray | "completed" |
| Awaiting approval | rgb(163, 113, 247) -- purple | (not tested in detail) |

## Issues Found

None. All 7 features pass their verification points.

## Overall Quality

All tested features are working correctly:

1. Mode indicator pill badges in StateBar use appropriate colors and labels for all three modes.
2. Terminal conversations show muted "completed" state, not green "ready".
3. Explore mode InputArea has the correct contextual placeholder.
4. New conversation page shows mode preview subtitle that reacts to directory changes.
5. Terminal banner displays system message context above the action button.
6. The propose_plan -> propose_task rename is complete in all source code.
7. "Direct" is used consistently instead of "SOLO" throughout the UI.
