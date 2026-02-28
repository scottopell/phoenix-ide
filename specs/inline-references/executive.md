# Inline References - Executive Summary

## Requirements Summary

Inline references let users embed file and skill pointers directly in their message input using three trigger patterns. `@src/main.rs` expands to include the file's full contents in what the AI sees. `/writing-style` loads a skill's instruction set into the AI's context, with optional trailing text substituted as `$ARGUMENTS`. `./src/auth.rs` autocompletes the path and sends it as literal text, leaving the AI to decide how and how much to read — no content is injected. All three triggers share an inline autocomplete dropdown for file/skill discovery. Expansion references (`@`, `/`) validate at send time and block on errors; path references (`./`) do not validate and never block.

## Technical Summary

Expansion references route through a `MessageExpander` layer in `send_chat`, producing `display_text` (stored, shown in history) and `llm_text` (delivered to LLM). File references wrap content in `<file path="...">` blocks. Skill references call `discover_skills()` from `src/system_prompt.rs` and perform `$ARGUMENTS` substitution. Path references (`./`) bypass the expander entirely — autocomplete is frontend-only. Two new endpoints feed autocomplete: `GET /api/conversations/:id/skills` and `GET /api/conversations/:id/files/search?q=`. Frontend adds a shared `InlineAutocomplete` overlay to `InputArea` with three modes (`expand`, `path`, `skill`), reusing `CommandPalette` keyboard nav.

## Status Summary

| Requirement | Status | Notes |
|---|---|---|
| **REQ-IR-001:** Include File Contents by Reference | ❌ Not Started | File read API exists; expansion layer does not |
| **REQ-IR-002:** Load Skill Context by Name | ❌ Not Started | `discover_skills()` exists; expansion layer does not |
| **REQ-IR-003:** Pass Additional Context to Skill Invocations | ❌ Not Started | Depends on REQ-IR-002 |
| **REQ-IR-004:** Discover Files via Inline Autocomplete | ❌ Not Started | Shared by `@` and `./` triggers |
| **REQ-IR-005:** Discover Skills via Inline Autocomplete | ❌ Not Started | Skills endpoint needed |
| **REQ-IR-006:** Preserve Original Shorthand in Conversation History | ❌ Not Started | Applies to `@` and `/` only; `./` is already literal |
| **REQ-IR-007:** Graceful Handling of Unresolvable Expansion References | ❌ Not Started | Applies to `@` and `/` only |
| **REQ-IR-008:** Reference Files by Path Without Expansion | ❌ Not Started | Frontend autocomplete only; no backend expansion |

**Progress:** 0 of 8 complete
