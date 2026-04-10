---
created: 2026-04-10
priority: p1
status: in-progress
artifact: pending
---

# terminal-spec

## Plan

# Terminal Spec — spEARS + Allium

## Summary

Write the full formal specification for the PTY-backed browser terminal feature. This creates `specs/terminal/` with four files: `requirements.md`, `executive.md`, `design.md`, and `terminal.allium`. No implementation code is written in this task — spec only.

Also file one follow-on task: `git-fetch-remote-branches`.

## Context

Scott provided a detailed KT document covering the full architecture. All design questions have been resolved through elicitation:

- **Unix user**: same as API server, no setuid needed
- **Max terminals**: exactly 1 per conversation (rejected connections return 409)
- **UI placement**: deferred to implementation
- **vt100 scraping**: v1 — server-side parser, agent-readable
- **WS auth**: same session mechanism as the rest of the API
- **Output persistence**: ephemeral only — tmux integration deferred
- **Agent access**: `read_terminal` tool exposing vt100 screen contents
- **Platform target**: Linux v1; macOS kqueue considerations deferred

## Files to Create

### `specs/terminal/requirements.md`

REQ-TERM-001 through REQ-TERM-012 covering:
- REQ-TERM-001: PTY-backed terminal per conversation
- REQ-TERM-002: Explicit shell environment construction (no server env inheritance)
- REQ-TERM-003: Exactly one terminal per conversation (duplicate connections rejected)
- REQ-TERM-004: Binary WebSocket framing (type-prefixed frames)
- REQ-TERM-005: Initial resize sent before first prompt
- REQ-TERM-006: Resize propagated via TIOCSWINSZ + SIGWINCH
- REQ-TERM-007: EIO treated as clean termination (not error)
- REQ-TERM-008: Master fd closed on WebSocket close → SIGHUP chain
- REQ-TERM-009: Child reaped via waitpid to prevent zombies
- REQ-TERM-010: vt100 parser fed every byte in order, in parallel with WebSocket send
- REQ-TERM-011: `read_terminal` agent tool returns parser screen contents
- REQ-TERM-012: Terminal torn down when conversation reaches terminal state

### `specs/terminal/executive.md`

One-page summary: what it is, why it exists, scope boundaries, build order, deferred items (tmux, OSC 133, macOS).

### `specs/terminal/design.md`

Implementation-level detail drawn from the KT doc:
- Full spawn path with exact syscall sequence (`openpty`, `fork`, `setsid`, `TIOCSCTTY`, `dup2`, `chdir`, `execvp`)
- Environment construction table
- WebSocket binary frame protocol (`0x00` data, `0x01` resize)
- Two-task async I/O model (reader: master→WS, writer: WS→master)
- vt100 parser integration (same-byte feed invariant, quiescence debounce 300ms)
- `TerminalHandle` struct
- Backpressure: bounded channel for output
- Known gotchas (EIO, orphan shells, zombies, initial size race, secret env leakage)
- Build order (PTY smoke test → WebSocket wiring → xterm.js → vt100 → OSC 133)
- Crate choices: `nix` (not `portable-pty`) for learning value; `vt100` for parser; axum WebSocket for transport

### `specs/terminal/terminal.allium`

Full formal Allium v3 spec:

```
-- allium: 3
-- Scope: PTY-backed browser terminal per conversation
-- Includes: session lifecycle, PTY spawn, WebSocket I/O, resize,
--           vt100 scraping, read_terminal agent tool, teardown
-- Excludes: UI placement, scrollback persistence, tmux, OSC 133, macOS
-- Requirement traceability: REQ-TERM-001 through REQ-TERM-012
-- Dependencies: bedrock/bedrock.allium
```

**Entities:**
- `Terminal` — status: `absent | active`; state-dependent fields (`master_fd`, `child_pid`, `dimensions`, `parser`) present only when `active`

**Config:**
- `quiescence_debounce: Duration = 300.ms`
- `output_channel_bound: Integer = 4096`

**Rules (12):**
1. `TerminalOpened` — WebSocket connects, PTY spawned, parser initialized
2. `ShellEnvironmentConstructed` — explicit env, no server env inheritance
3. `DuplicateTerminalRejected` — 409 if terminal already active
4. `PtyOutputForwarded` — bytes → WebSocket frame AND parser (same bytes, in order)
5. `InitialResizeSent` — resize before first prompt on connect
6. `ResizeApplied` — TIOCSWINSZ + parser resize, dimensions kept in sync
7. `UserInputForwarded` — data frame → master fd write
8. `ShellExited` — EIO → absent + WebSocket close + waitpid
9. `UserClosedTerminal` — WebSocket close → master fd close → SIGHUP chain
10. `TerminalScreenRead` — agent reads vt100 screen contents
11. `PtyQuiesced` — debounced quiescence signal (optimisation hint)
12. `TerminalAbandonedWithConversation` — conversation terminal state → teardown

**Invariants:**
- `OneTerminalPerConversation` — count of active terminals per conversation ≤ 1
- `ParserDimensionSync` — parser dimensions always match terminal dimensions
- `ParserFedEveryByte` — structural: same rule ensures both WS send and parser feed

**Surfaces:**
- `UserTerminalAccess` — exposes `terminal.status`; provides `UserOpensTerminal` (when absent), `UserClosedTerminal` (when active)
- `AgentTerminalAccess` — provides `AgentRequestsScreenContents` (when active)

**Deferred:**
- `TmuxIntegration.named_sessions`
- `ShellIntegration.osc133_markers`
- `macOsKqueueSupport`

## Follow-on Task to File

**`08657-p2-ready--git-fetch-remote-branches.md`** — Add a Fetch button to the managed conversation branch picker. On fetch, run `git fetch` against the remote and repopulate the branch list including the remote's default branch (e.g. `origin/main`) so users can start a conversation at the upstream tip without needing a local tracking branch.

## Acceptance Criteria

- [ ] `specs/terminal/requirements.md` — all 12 REQ-TERM-NNN entries present with status
- [ ] `specs/terminal/executive.md` — one-pager, scope, deferred items, build order
- [ ] `specs/terminal/design.md` — full KT doc content structured as spEARS design
- [ ] `specs/terminal/terminal.allium` — valid Allium v3; 0 open questions
- [ ] `tasks/08657-p2-ready--git-fetch-remote-branches.md` filed
- [ ] `./dev.py tasks validate` passes


## Progress

