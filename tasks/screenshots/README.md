# UI screenshots — tasks/ready review

Captured 2026-04-14 via `agent-browser` (Chromium via CDP) against
`./dev.py up` running the **mock** LLM provider. All shots are 1600×1000
unless noted.

The goal was to walk the home screen and main conversation UI while
cross-checking the UI-focused tasks in `tasks/*-ready--*.md`.

## Shots

| # | File | What it shows |
|---|---|---|
| 01 | `01-home.png` | First paint on `/` with default cwd `/root`. Model dropdown is empty, "0 recommended" — the only available model is `mock` and it is hidden by default. |
| 02 | `02-home-phoenix-dir.png` | Directory retyped to `/home/user/phoenix-ide`. Now a second mode appears: **Managed (BETA)** alongside **Direct**. Model dropdown still empty. |
| 03 | `03-show-all-models.png` | Checked "Show all models (1)" to unhide the mock model. Model dropdown now resolves to `mock`. Send button still disabled until a message is typed. |
| 07 | `07-home-empty-sidebar.png` | Sidebar after a conversation has been archived. Project filter pills (**All** / **phoenix-ide**) and an **Archived (1)** button are visible. |
| 08 | `08-project-view.png` | `phoenix-ide` project filter selected. |
| 09 | `09-archived-view.png` | Archived conversations view. Conversation card shows title / mode pill / "just now" / message count. |
| 11 | `11-home-configured.png` | Home form fully configured (`/home/user/phoenix-ide`, mock model, Direct mode). |
| 12 | `12-fresh-conversation.png` | Fresh conversation right after sending "hello". Note the red banner: `Invalid transition: No transition from Idle with event UserCancel { reason: None }` — appeared when `POST /cancel` was issued after the mock had already replied and state had returned to idle. See task `24682`. |
| 13 | `13-tasks-section-expanded.png` | The collapsible **Tasks** section under the file explorer expanded — READY (29), BLOCKED (3), BRAINSTORMING (5), DONE (209), WONT-DO (7). |
| 14 | `14-task-file-opened.png` | A task file (`08605 auto-scroll-on-new-messages`) opened in the prose reader pane. The reader column is extremely narrow — this is the split-pane mentioned in task 08654. |
| 15 | `15-light-mode.png` | Light theme forced via `data-theme="light"` on `<html>`. **Partial coverage**: chat area, file tree and main input switch to light, but the left conversation-list sidebar, the **FILES** header, the tool-tab row, and the terminal stay dark. |

## UI observations that line up with ready tasks

- **0-model deadlock (`08609 model-null-display-fix`).** With only the mock
  provider available, the recommended-models filter is empty, so the
  dropdown renders blank until you tick "Show all models". Users who don't
  notice the checkbox are stuck with a disabled Send button. See shots 01 →
  03. The task description is about StateBar displaying `"null"`, but the
  underlying root cause (no resolved model at creation time) is the same
  problem.

- **Auto-scroll / "New Messages" chip (`08605 auto-scroll-on-new-messages`).**
  When the mock provider dumped ~800 messages, a "New Messages" chip
  appeared mid-chat — the affordance exists, but the message list is not
  actually following new content. Visible in the earlier dense shot; the
  small fresh conversation (shot 12) doesn't reach the threshold.

- **Sidebar Files section + scroll (`08653 sidebar-files-section-and-scroll-behavior`).**
  The "collapsible Skills / MCP / Tasks sections" described in that task
  actually live inside the FileExplorer panel, not the sidebar. Shots 13
  and 14 show the Tasks section expanding and pushing the rest of the
  tree, which matches the "expand pushes headers off-screen" issue in the
  task write-up.

- **Split-pane prose reader (`08654 split-pane-prose-reader-chat`).** Shot
  14 — the opened task file reader column is fixed to the file-explorer
  width (~220px), making markdown almost unreadable. This is the task's
  core complaint.

- **Terminal panel hardcoded dark (task 24681).** Shot 15 initially
  looked like multiple panels stayed dark in light mode. After computing
  backgrounds on every element in the DOM, only `.terminal-panel` and
  `.xterm-viewport` are actually opaque dark (`#1a1a1a`). Everything
  else is light-theme-correct. The sidebar / file tree shading I thought
  was a bug was actually `#f6f8fa` vs `#ffffff` at thumbnail scale.

- **Cancel-on-idle raw Debug error (task 24682).** Shot 12's red banner
  reads `"Invalid transition: No transition from Idle with event UserCancel
  { reason: None }"` (authoritative text pulled from the live DOM via
  `agent-browser eval`). Repro: send a message to mock, let it settle to
  idle, then `POST /cancel` — the API returns `{ok:true}` but the SSE
  path emits the raw Rust Debug rendering.

- **Mock runaway → backend, not UI (tasks 24684 + 24680).** The 797→829
  message conversation I hit was a real backend loop, not a UI duplication
  bug. SQLite had 414 distinct agent rows + 414 distinct tool rows, each
  with unique `mock_toolu_*` ids. Root causes: (a) `read_file` missing
  from the Direct-mode tool registry (24684, renumbered from 24679 during
  rebase to avoid colliding with main's shell-integration task),
  (b) no iteration cap on parent-conversation tool_use loops (24680).
  User-reported streaming duplication is a separate issue being
  investigated.

## How I reproduced this

```bash
./dev.py up                        # Phoenix @ 8033, Vite @ 8042
npm i -g agent-browser             # CDP-based browser CLI
agent-browser --executable-path /opt/pw-browsers/chromium-1194/chrome-linux/chrome \
  open http://localhost:8042/
agent-browser set viewport 1600 1000
agent-browser snapshot -i          # interactive refs
agent-browser screenshot tasks/screenshots/NN-name.png
```

Browser daemon persists across commands, so `open → snapshot → click →
screenshot` can run in a single shell. `agent-browser close --all` to
tear down between runs.
