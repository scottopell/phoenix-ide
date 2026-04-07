# Inline References - Executive Summary

## Requirements Summary

Inline references let users embed file and skill pointers directly in their message input using three trigger patterns. `@src/main.rs` expands to include the file's full contents in what the AI sees. `/writing-style` loads a skill's instruction set into the AI's context, with optional trailing text substituted as `$ARGUMENTS`. `./src/auth.rs` autocompletes the path and sends it as literal text, leaving the AI to decide how and how much to read — no content is injected. All three triggers share an inline autocomplete dropdown for file/skill discovery. Expansion references (`@`, `/`) validate at send time and block on errors; path references (`./`) do not validate and never block.

## Technical Summary

Expansion references route through a `MessageExpander` layer in `send_chat`, producing `display_text` (stored, shown in history) and `llm_text` (delivered to LLM). File references wrap content in `<file path="...">` blocks. Skill references call `discover_skills()` from `src/system_prompt.rs` and perform `$ARGUMENTS` substitution. Path references (`./`) bypass the expander entirely — autocomplete is frontend-only. Two new endpoints feed autocomplete: `GET /api/conversations/:id/skills` and `GET /api/conversations/:id/files/search?q=`. Frontend adds a shared `InlineAutocomplete` overlay to `InputArea` with three modes (`expand`, `path`, `skill`), reusing `CommandPalette` keyboard nav.

## Status Summary

| Requirement | Status | Notes |
|---|---|---|
| **REQ-IR-001:** Include File Contents by Reference | ✅ Complete | `src/message_expander.rs`; wired in `src/api/handlers.rs` |
| **REQ-IR-002:** Load Skill Context by Name | ✅ Complete | `/skill-name` expansion via `discover_skills()` in `message_expander.rs` |
| **REQ-IR-003:** Pass Additional Context to Skill Invocations | ✅ Complete | `$ARGUMENTS` substitution in `message_expander.rs` |
| **REQ-IR-004:** Discover Files via Inline Autocomplete | ✅ Complete | `InlineAutocomplete.tsx` (`expand`/`path` modes); `GET /api/conversations/:id/files/search` |
| **REQ-IR-005:** Discover Skills via Inline Autocomplete | ✅ Complete | `InlineAutocomplete.tsx` (`skill` mode); `GET /api/conversations/:id/skills` |
| **REQ-IR-006:** Preserve Original Shorthand in Conversation History | ✅ Complete | `display_text`/`llm_text` separation in DB schema, state machine, and handlers |
| **REQ-IR-007:** Graceful Handling of Unresolvable Expansion References | ✅ Complete | `ExpansionError` enum (backend), HTTP 422, `ExpansionError` class in `ui/src/api.ts` |
| **REQ-IR-008:** Reference Files by Path Without Expansion | ✅ Complete | `./` mode inserts literal path only; no server-side expansion |

**Progress:** 8 of 8 complete
