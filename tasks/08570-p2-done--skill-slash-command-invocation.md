---
created: 2025-01-28
priority: p2
status: done
artifact: completed
---

# Implement skill slash-command invocation (REQ-IR-002, REQ-IR-003, REQ-IR-005)

## Summary

Implement the `/skill-name` inline reference pattern. See `specs/inline-references/` for full requirements and design.

## Scope

This task covers the `/skill` half of the inline references feature:

- **REQ-IR-002** — `/skill-name` send-time context loading (SKILL.md injected into LLM message)
- **REQ-IR-003** — `$ARGUMENTS` substitution for trailing text after `/skill-name`
- **REQ-IR-005** — `/` autocomplete trigger in `InputArea` with `argument-hint` ghost text

REQ-IR-006 (`display_text`/`llm_text` separation) and REQ-IR-007 (block on unresolvable reference) apply here too and should be implemented as part of the shared `MessageExpander` — coordinate with Task 546 to avoid duplication.

Advanced skill features (`context: fork`, `allowed-tools`, etc.) are tracked in Task 571.

## Existing infrastructure to reuse

- `discover_skills(working_dir)` — `src/system_prompt.rs` (skill walk + frontmatter parse)
- `CommandPalette` keyboard nav pattern — reuse for the `/` dropdown

## What to build

1. Extend `parse_skill_frontmatter` / `SkillMetadata` to carry `argument_hint`
2. `GET /api/conversations/:id/skills` — exposes `discover_skills()` for frontend autocomplete
3. `MessageExpander` skill branch — detects `/skill-name`, loads SKILL.md, performs `$ARGUMENTS` substitution
4. `/` trigger in `InputArea` `InlineAutocomplete` (coordinate shared component with Task 546)

## Acceptance Criteria

- [ ] `GET /api/conversations/:id/skills` returns `[{ name, description, argument_hint | null }]`
- [ ] Typing `/` at the start of input opens a skill autocomplete dropdown
- [ ] Dropdown shows skill name, description, and `argument-hint`; filters by fuzzy match
- [ ] Selecting a skill inserts `/skill-name ` and shows `argument-hint` as ghost text
- [ ] Sending `/writing-style` loads full SKILL.md content into the LLM message
- [ ] Sending `/writing-style help me write` substitutes `$ARGUMENTS` with the trailing text
- [ ] Trailing text on a skill without `$ARGUMENTS` is appended as `ARGUMENTS: <value>`
- [ ] Unknown skill name blocks send with an inline error listing available skills
- [ ] Stored message retains `/skill-name …` shorthand; LLM receives the expanded form
- [ ] `./dev.py check` passes
