---
created: 2026-02-15
priority: p2
status: ready
spec: specs/inline-references/
---

# Implement `@file` file content inclusion (REQ-IR-001, REQ-IR-004, REQ-IR-006, REQ-IR-007)

## Summary

Implement the `@path/to/file` inline reference pattern. See `specs/inline-references/` for full requirements and design.

## Scope

This task covers the `@` half of the inline references feature:

- **REQ-IR-001** — `@path` send-time expansion (file contents injected into LLM message)
- **REQ-IR-004** — `@` autocomplete trigger in `InputArea` (shared file picker; `./` trigger in Task 572)
- **REQ-IR-006** — `display_text` / `llm_text` separation (`@src/main.rs` stored, expanded form delivered)
- **REQ-IR-007** — block send on unresolvable `@` reference

`./` path references (REQ-IR-008) are tracked in Task 572. Skill invocation (REQ-IR-002, REQ-IR-003, REQ-IR-005) is tracked in Task 570.

## What to build

1. `GET /api/conversations/:id/files/search?q=&limit=` — gitignore-aware recursive search (see design.md)
2. `MessageExpander` layer in `send_chat` — resolves `@` tokens, produces `ExpandedMessage { display_text, llm_text }`
3. `InlineAutocomplete` overlay in `InputArea` — `@` trigger, file results, keyboard nav

## Acceptance Criteria

- [ ] Typing `@` in the message input opens a file autocomplete dropdown
- [ ] Dropdown filters by fuzzy match as the user continues typing
- [ ] Selecting a file inserts `@path/to/file` at the cursor
- [ ] Sending a message with `@path/to/file` delivers file contents wrapped in `<file path="...">` to the LLM
- [ ] Conversation history shows the original `@path/to/file` shorthand, not the expanded contents
- [ ] A `@` reference to a non-existent or unreadable file blocks send with an inline error
- [ ] Binary files are excluded from expansion (show in autocomplete, reject if selected for `@`)
- [ ] `./dev.py check` passes
