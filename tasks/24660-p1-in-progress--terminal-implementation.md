---
created: 2026-04-10
priority: p1
status: done
artifact: pending
---

# terminal-implementation

## Plan

# Terminal Implementation — Overseer Task

## Summary

Implement the PTY-backed browser terminal feature as defined in `specs/terminal/`. Six serial subagents, each completing one build phase before the next begins. I (the overseer) coordinate, verify each phase, and hand state to the next subagent.

## Context

- Spec: `specs/terminal/` — 4 files, 14 requirements, 0 open questions, 0 implementation started
- Allium: `specs/terminal/terminal.allium` — 11 rules, 3 invariants, fully resolved
- Design decisions locked in:
  - UI: terminal panel **below conversation chat**, toggled by a "Terminal" button in the conversation header
  - `read_terminal` tool: accepts `wait_for_quiescence: bool` parameter (default `true`)
  - `ConversationBecameTerminal` bedrock event: implement as prerequisite (Task 1) before any terminal code
  - Build order: 6 serial tasks, each depends on the previous
- Existing codebase already has `nix` in `Cargo.toml` (needs `pty` feature added), no `vt100` crate yet, axum missing `ws` feature

---

## The 6 Tasks

### Task 1 — `ConversationBecameTerminal` Bedrock Event (Prerequisite)

**Scope:** Bedrock-only change. No terminal code yet.

- Add a `ConversationBecameTerminal` lifecycle event emitted by the runtime executor whenever a conversation's `is_terminal()` transitions from `false` to `true`
- Triggered by transitions into: `Completed`, `Failed`, `ContextExhausted`, `Terminal` states
- Event must be subscribable by other subsystems (the terminal teardown in Task 5 will wire to it)
- Also fix the existing bug tracked in `tasks/08659-p2-ready--fix-is-terminal-missing-context-exhausted.md` — `is_terminal()` currently returns false for `ContextExhausted`; fix this as part of the same change
- Update `specs/bedrock/bedrock.allium` to record the new event
- Update task 08659 status to done

**Done when:** `./dev.py check` passes; `is_terminal()` returns true for all four terminal states; the event is emitted and observable in test.

---

### Task 2 — PTY + WebSocket Backend

**Scope:** Rust backend only. No frontend work yet.

**Cargo.toml changes:**
- `nix` features: add `"pty"` alongside existing `"signal"`, `"process"`
- Add `vt100 = "0.15"` (or latest stable)
- Add `"ws"` to axum features

**Implementation:**
- `src/terminal.rs` + `src/terminal/` module: `TerminalHandle` struct (`master_fd: OwnedFd`, `child_pid: Pid`), with `Drop` that closes `master_fd` (triggers SIGHUP chain)
- PTY spawn sequence: `openpty` → `fork` → child: `setsid`, `TIOCSCTTY`, `dup2(slave, 0/1/2)`, `close(slave)`, `chdir(cwd)`, `execvp($SHELL -i)` with explicit env; parent: `close(slave)`, store master
- Explicit env construction (`TERM=xterm-256color`, `COLORTERM=truecolor`, `HOME`, `USER`, `SHELL`, `PATH`, `LANG=en_US.UTF-8`) — no server env inheritance
- Active terminal registry: `Arc<Mutex<HashMap<ConversationId, TerminalHandle>>>` added to `AppState`
- WebSocket endpoint: `GET /api/conversations/:id/terminal` (axum WS upgrade)
  - Auth: goes through existing session middleware (REQ-TERM-013; no new auth code needed)
  - 409 guard: atomic check-and-insert against registry before spawn
  - Binary frame protocol: `0x00` = PTY data (bidirectional), `0x01` = resize (`u16be cols`, `u16be rows`)
  - Initial resize handshake: wait for first client resize frame before entering normal I/O
  - Task A: `loop { read(master_fd) → WebSocket binary frame }` — EIO = clean shutdown (`tracing::debug`, not error); calls `waitpid` to reap child
  - Task B: `loop { WebSocket frame → write(master_fd) | ioctl(TIOCSWINSZ) }` — handles both data and resize frames
  - Both tasks hold `Arc<TerminalHandle>`; either exiting shuts down the other
  - Bounded output channel (`output_channel_bound = 4096`) for backpressure — never drop bytes
  - Text frame received → close connection with error (binary-only protocol)
- Register endpoint in axum router

**Done when:** `./dev.py check` passes; manual test with `websocat` can open a shell, type commands, receive output, resize, and close cleanly with no orphan processes.

---

### Task 3 — xterm.js Frontend

**Scope:** React/TypeScript frontend only. Connects to the Task 2 backend.

**npm dependencies:**
- `xterm` (latest stable)
- `xterm-addon-fit`

**Implementation:**
- `ui/src/components/TerminalPanel.tsx`: xterm.js instance in a `div`, `FitAddon` attached, binary WebSocket connection to `/api/conversations/:id/terminal`
  - On connect: FitAddon sends initial resize frame (type `0x01` + cols/rows as `u16be`) as first message — this satisfies REQ-TERM-005
  - On `data` event from xterm.js: send binary frame (`0x00` prefix + bytes)
  - On binary frame from server (`0x00` prefix): write bytes to xterm.js
  - On window/panel resize: send resize frame via FitAddon
  - On WebSocket close: display a "Terminal closed" notice inline in the panel
  - On 409 response to upgrade: show "Terminal already open in another tab"
- `ui/src/pages/ConversationPage.tsx`: add a **Terminal** toggle button in the conversation header
  - Button only shown for non-terminal-state conversations
  - Clicking toggles the `TerminalPanel` open/closed below the chat area
  - When panel opens: `TerminalPanel` mounts and initiates the WebSocket connect + PTY spawn
  - When panel closes: WebSocket closes (triggers master fd close → SIGHUP → shell exits)
  - Panel is a fixed-height strip below the chat (resizable later; fixed height for now, ~40% viewport)
- Terminal button visual state: active when panel is open

**Done when:** `./dev.py check` passes; can open terminal in browser, type `ls`, see output, resize window and see terminal reflow, close terminal panel cleanly.

---

### Task 4 — vt100 Parser Layer

**Scope:** Rust backend. Adds the parser alongside the existing I/O relay.

**Implementation:**
- Add `vt100::Parser` to `TerminalHandle` (or held alongside it in the session state): initialized with `(rows, cols)` from the initial resize frame
- `apply_resize(master_fd, parser, dims)` helper: calls `ioctl(TIOCSWINSZ)` AND `parser.set_size(rows, cols)` in the same function — structural guarantee of `ParserDimensionSync` invariant
- Thread `apply_resize` through both the initial resize path (Task 2) and all subsequent resize frames
- Task A read loop: after sending bytes to WebSocket, call `parser.process(&bytes)` — same bytes, same order, no gaps. This is the `PtyOutputForwarded` rule from the Allium spec; both the WS send and parser feed are in the same handler with no conditional path that skips either
- Quiescence detection: after each read, if no bytes arrive within `300ms`, emit a `PtyIsQuiescent` signal (e.g., a tokio channel or `Arc<AtomicBool>`) — used by the `read_terminal` tool in Task 6
- Parser protected behind `Arc<Mutex<vt100::Parser>>` so the `read_terminal` tool can read it from a different task

**Done when:** `./dev.py check` passes; parser is live alongside output relay; `apply_resize` called on every resize; no byte drops.

---

### Task 5 — Conversation Teardown

**Scope:** Wire REQ-TERM-012. Depends on Task 1 (`ConversationBecameTerminal` event) and Task 2 (terminal registry).

**Implementation:**
- In `src/runtime/executor.rs` (or wherever `ConversationBecameTerminal` is emitted after Task 1): subscribe the terminal registry to this event
- When event fires for a conversation: look up registry, call `master_fd` close on the `TerminalHandle` (drop it from the registry) → SIGHUP chain fires → shell exits → Task A reads EIO → `waitpid` called
- No new cleanup code needed beyond removing from registry (Drop handles the rest)
- Update `specs/terminal/executive.md`: mark REQ-TERM-012 as ✅
- Confirm no orphan shells remain after conversation completes/fails/abandons via integration test or manual verification

**Done when:** `./dev.py check` passes; completing or abandoning a conversation with an active terminal tears it down (shell exits, no zombies, registry entry removed).

---

### Task 6 — `read_terminal` Agent Tool + Status Wrap-up

**Scope:** Final integration. All 14 requirements become complete.

**Implementation:**
- `src/tools/read_terminal.rs`: implement `Tool` trait
  - `name()`: `"read_terminal"`
  - `description()`: explain it returns the current terminal screen contents; mention waiting for quiescence gives better results after a command
  - `input_schema()`: `{ wait_for_quiescence: bool }` — if `true` (default), wait up to 5s for the quiescence signal from Task 4 before returning; if `false`, return immediately
  - `run()`: look up terminal for the calling conversation; if absent → `ToolOutput::error("no terminal is open for this conversation")`; if active → acquire parser lock, call `parser.screen().contents()`, return as `ToolOutput::success(text)`
- Register in `src/tools.rs` → `ToolRegistry::new_with_options()`
- Add spec stub `specs/terminal/read_terminal_tool.md` if tool specs convention requires it (check existing tools)
- Update `specs/terminal/executive.md`: mark all 14 REQ-TERM-* as ✅
- Update `tasks/24657-p1-in-progress--terminal-spec.md` → status `done`
- Update `tasks/24657-p1-wont-do--terminal-spec.md` → delete or reconcile the duplicate (task validation will catch this)
- Run `./dev.py tasks validate` and fix any issues

**Done when:** `./dev.py check` passes; an LLM agent can call `read_terminal` and receive the current terminal screen; all 14 requirements marked complete in executive.md; task validation passes.

---

## Overseer Coordination Pattern

Each task runs as a `work` mode subagent. Before launching the next subagent, I:

1. Read the key output files from the completed task to verify correctness
2. Check that `./dev.py check` passed (subagent must confirm this)
3. Confirm the specific acceptance criteria above are met
4. Pass any relevant context (e.g., struct names, channel types, event signatures) into the next subagent's prompt

If a subagent fails or produces incorrect output, I diagnose and re-run (or run a corrective subagent) before proceeding.

---

## Acceptance Criteria (Final)

- [ ] `ConversationBecameTerminal` event exists and fires correctly for all 4 terminal states
- [ ] `is_terminal()` returns true for `ContextExhausted` (bug fix)
- [ ] Terminal button in conversation header toggles panel open/closed
- [ ] Terminal panel appears below chat, connects via binary WebSocket
- [ ] Interactive programs (vim, htop) work correctly
- [ ] Resize reflows terminal and xterm.js stays in sync
- [ ] EIO logged at debug, not error
- [ ] No orphan shells or zombies after any close path
- [ ] vt100 parser fed every byte in order, dimensions always in sync
- [ ] Conversation teardown cascades to terminal
- [ ] `read_terminal` tool registered, callable by agents, supports `wait_for_quiescence`
- [ ] All 14 REQ-TERM-* marked ✅ in `specs/terminal/executive.md`
- [ ] `./dev.py check` passes (clippy, fmt, tests, task validation)


## Progress

