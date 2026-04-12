---
created: 2026-04-10
priority: p1
status: in-progress
artifact: src/terminal/, ui/src/components/TerminalPanel.tsx
---

# Terminal Implementation

Implementing the PTY-backed browser terminal as specified in `specs/terminal/`. Six serial phases.

## Progress

- [x] Task 1: ConversationBecameTerminal bedrock event + is_terminal() fix
- [x] Task 2: PTY + WebSocket backend
- [x] Task 3: xterm.js frontend
- [x] Task 4: vt100 parser layer
- [x] Task 5: Conversation teardown
- [x] Task 6: read_terminal agent tool

## Spec

See `specs/terminal/` for requirements, design, and Allium behavioral spec.
