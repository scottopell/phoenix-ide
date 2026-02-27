---
created: 2025-01-28
priority: p2
status: ready
---

# Skill slash-command invocation: `/skill-name` message expansion

## Summary

Users cannot currently invoke skills directly — they must ask the LLM to "use the writing-style skill" in natural language. This task adds `/skill-name` as a first-class input gesture: the frontend shows a slash-command autocomplete, and the backend loads the skill's full context into the message before it reaches the LLM.

**Mental model:** invoking a skill is always *context loading* — the SKILL.md content is injected so the LLM operates under those instructions. Any text typed after `/skill-name` is optionally available inside the skill content via `$ARGUMENTS` substitution, but that is additive; the primary action is always loading context.

## Related

- **Task 546** (`@file` autocomplete) — parallel pattern: both are send-time message expansion. Implement the shared `MessageExpander` layer here or in 546; they should share infrastructure.
- **Task 571** — advanced skill features deferred from this task (`context: fork`, subagent execution, `!`command`` injection, `allowed-tools`).

## Existing infrastructure (reuse, don't rebuild)

| Component | Location | Status |
|---|---|---|
| Skill discovery (walk fs, parse frontmatter) | `src/system_prompt.rs` → `discover_skills()` | ✅ done |
| `$ARGUMENTS` substitution semantics | Defined in claude-code spec above | design reference |
| Skill catalog in system prompt | `build_system_prompt()` | ✅ done |
| Fuzzy autocomplete dropdown UI pattern | `CommandPalette/` | ✅ done |

## What needs to be built

### 1. Backend — skill resolution endpoint

New route (or extend conversation detail response):

```
GET /api/conversations/:id/skills
→ [{ name, description, argument_hint, path }]
```

Calls `discover_skills(conversation.working_dir)` — already written, just needs to be exposed.

### 2. Backend — send-time expansion

In `send_chat` (or a new `MessageExpander` layer called before `Event::UserMessage` is emitted), detect a slash-command prefix and expand it:

```
/writing-style
  ↓
[full SKILL.md content]

/writing-style help me draft a proposal
  ↓
[SKILL.md content with $ARGUMENTS replaced by "help me draft a proposal"]
```

Expansion rules (matching claude-code semantics):
- Detect `/skill-name` at start of message; any text after the name is captured as the arguments string
- Load the SKILL.md content — this is the primary action (context loading)
- If the SKILL.md body contains `$ARGUMENTS`, `$ARGUMENTS[N]`, or `$N` placeholders and arguments were provided, substitute them
- If arguments were provided but `$ARGUMENTS` is not present in content, append `ARGUMENTS: <value>` at end
- If no arguments are provided, no substitution occurs; the skill loads as-is
- Unknown skill name → return 400 with available skill names
- The original `/skill-name …` text is preserved as the stored user message; the expanded form is what goes to the LLM (keep them separate — don't overwrite the DB message)

### 3. Frontend — `/` trigger in InputArea

- Typing `/` at the start of input (or after whitespace) opens an autocomplete dropdown
- Filters skills by fuzzy match on name as user continues typing
- Shows `name`, `description`, and `argument-hint` (if present) per row
- Select with Enter/click → inserts `/skill-name ` at cursor, dismisses dropdown
- Escape dismisses without inserting
- Reuses Command Palette keyboard nav pattern

### 4. Frontend — argument hint

Once a valid `/skill-name` is committed, if the skill's frontmatter includes `argument-hint`, show it as placeholder/ghost text in the textarea (e.g. `[filename] [format]`). This communicates to the user that additional context is welcome, without implying it is required.

## Expansion architecture note

Both this task and 546 (`@file`) expand user message text before LLM delivery. Rather than two independent hacks in the HTTP handler, introduce a single `expand_message(text, working_dir) -> ExpandedMessage` function that handles all expansion rules in one pass:

```rust
struct ExpandedMessage {
    /// What the LLM sees (expanded)
    llm_text: String,
    /// What is stored / shown in UI (original shorthand)
    display_text: String,
}
```

This is the right place to add future expansion forms.

## Acceptance Criteria

- [ ] `GET /api/conversations/:id/skills` returns available skills for the conversation's working directory
- [ ] Typing `/` in InputArea opens skill autocomplete dropdown
- [ ] Dropdown shows skill name, description, and `argument-hint` (if present in frontmatter)
- [ ] Selecting a skill inserts `/skill-name ` at cursor and shows `argument-hint` as ghost text
- [ ] Sending `/writing-style` (no extra text) loads the full SKILL.md content into the LLM message
- [ ] Sending `/writing-style help me write` loads SKILL.md and substitutes `$ARGUMENTS` with the trailing text
- [ ] Skills without `$ARGUMENTS` in their content receive trailing text appended as `ARGUMENTS: <value>`
- [ ] Skills with no trailing text load unmodified — no substitution, no appended text
- [ ] Unknown skill name returns a user-visible error, not a silent no-op
- [ ] Stored DB message retains the original `/skill-name …` shorthand (display_text), not the expanded form
- [ ] `./dev.py check` passes

## Out of Scope (tracked in Task 571)

- `disable-model-invocation` / `user-invocable` frontmatter fields
- `context: fork` subagent execution from a skill
- `agent: <name>` subagent selection
- `!\`command\`` dynamic context injection
- `allowed-tools` scoping per skill
