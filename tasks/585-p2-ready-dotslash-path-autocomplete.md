---
created: 2025-01-28
priority: p2
status: ready
spec: specs/inline-references/
---

# Implement `./path` file path autocomplete (REQ-IR-004, REQ-IR-008)

## Summary

Implement the `./path` inline reference pattern. See `specs/inline-references/` for full requirements and design.

## Scope

- **REQ-IR-008** — `./` triggers file autocomplete; selected path inserted as literal text, no send-time expansion
- **REQ-IR-004** (partial) — `./` as a second trigger on the shared `InlineAutocomplete` component

No `MessageExpander` involvement. This is entirely a frontend autocomplete assist — the `./path/to/file` text the user sends is received by the agent as-is.

## What to build

1. Add `./` trigger detection to `InputArea` (alongside `@` from Task 546)
2. Open the shared `InlineAutocomplete` overlay in `path` mode (same file search endpoint as `@`)
3. On selection, insert `./path/to/file` as plain text — no special marker, no validation at send time

## Acceptance Criteria

- [ ] Typing `./` anywhere in the message input opens the file autocomplete dropdown
- [ ] Dropdown filters by fuzzy match as the user continues typing
- [ ] Selecting a file inserts `./path/to/file` at the cursor as plain text
- [ ] Sending a message with `./path/to/file` delivers that literal string to the LLM unchanged
- [ ] No send-time validation or blocking occurs for `./` references
- [ ] `./dev.py check` passes

## Dependencies

Task 546 should land first (or alongside) — it builds the `InlineAutocomplete` component and the file search endpoint this task reuses.
