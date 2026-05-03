---
created: 2026-05-03
priority: p3
status: ready
artifact: src/message_expander.rs
---

<!--
ID 27101 chosen above 27100. Created without `taskmd new` since the binary
isn't installed; run `./dev.py tasks fix` if reallocation needed.
-->

# Skill invocation via `$skillname` mentions, alongside `/skill`

## Problem

Phoenix invokes skills via `/skillname` slash prefix
(`src/message_expander.rs:266-289`). It works, but it grabs an entire
message — `/skillname rest of message` is treated as one skill call with
"rest of message" as args. That makes natural composition awkward:

- "please review using $review and $lint" — currently impossible
- "/skill build && /skill deploy" — only the first match fires
- A user writing prose around a single skill mention has to either
  surrender the prose or quote it after the slash command

Codex CLI parses `$skillname` mentions in user input and triggers a
mid-turn skill body injection per mention
(`codex-rs/core-skills/src/injection.rs`). Pi has the same `/skill:name`
structure phoenix has, plus skill commands. Phoenix would benefit from
the looser `$` mention syntax for prose composition.

The user explicitly asked for this as an alternative trigger to `/`, not
a replacement.

## Goal

`$skillname` in a user message expands the same way `/skillname` does
today, parsed at the same point in the pipeline (`message_expander`).
`/skillname` continues to work unchanged.

## Design

### Tokenizer change

`tokenize_references(text, &['/', '@'])` already exists. Add `$` as a
third sigil. Token rules for `$`:

- Must be at start of line OR preceded by whitespace (so `$PATH` inside
  inline code or `$ARGUMENTS` echoed from a shell command doesn't trip
  it — though see "open question" below).
- Token body matches existing skill name regex (alphanumerics, hyphens,
  underscores, `:` for namespaced skills like `allium:distill`).
- Like `/`, must match a discovered skill name to count — otherwise the
  `$foo` is left as plain text.

### Expansion semantics — V1: replace-message (matches `/`)

Same as today's `/skillname` behavior. First `$skillname` token in the
message is treated as the invocation; the rest of the message becomes
its args. Other `$skillname` tokens in the same message are left as
plain text.

This keeps the V1 change small and fully consistent with existing
`/skill` semantics. **No new behavior, just an alternative sigil.**

### Expansion semantics — V2 (out of scope for this task, noted)

Codex-style multi-mention with inline composition. Each `$skillname`
mention triggers its own skill body injection; the original prose is
preserved. Allows "please review using $review then $lint".

This is a richer change — needs to decide whether bodies are inlined
into the user message, appended as separate user messages, or appended
as system fragments. Spec it in a follow-up task once V1 lands and we
see real usage.

### Conflict avoidance

Two collision sources to handle:

1. **Skill body argument substitution** uses `$ARGUMENTS`, `$1`,
   `$ARGUMENTS[N]` (`src/skills.rs:98-122`). This runs INSIDE skill
   bodies during invocation, never on user input. No collision —
   different context, different code path.
2. **Shell variables in user prose** (`$PATH`, `$HOME`). Mitigated by
   "must match a discovered skill" rule. If `$home` is not a skill
   name, the token stays as text.

### Code-fence / inline-code masking

The existing `masked_code_ranges` helper
(`src/message_expander.rs:95`) already excludes tokens inside backtick
fences. Reuse it for `$` mentions so `$ARGUMENTS` inside an
example code block in user prose doesn't get parsed.

## Acceptance criteria

- [ ] `tokenize_references` accepts `$` as a third sigil with the same
      token-body rules as `/`
- [ ] `expand()` treats a `$skillname` mention identically to
      `/skillname` (V1 semantics: first match wins, rest of message
      becomes args, single skill per message)
- [ ] `$skillname` is ignored if it doesn't match a discovered skill
      (left as plain text in the LLM message)
- [ ] `$skillname` inside backtick-fenced code is ignored
- [ ] Existing `/skillname` tests all pass unchanged
- [ ] New tests: `$skillname` no args, `$skillname args`, `$unknown`
      passes through, `$skill` inside ` ``` ` block ignored, both
      `/skillname` and `$skillname` discoverable in same message picks
      the FIRST one (whichever sigil)
- [ ] UI: `/skill` slash-command picker continues to work; consider
      whether `$` should also trigger autocomplete (probably yes; out
      of scope here unless trivial)
- [ ] `ExpansionError::SkillInvocationFailed.reference()` returns the
      original sigil + token (so error messages match what the user typed)
- [ ] Spec update in `specs/skills/` noting both sigils as valid

## Open questions to resolve before implementation

1. **Whitespace requirement before `$`?** Strict ("only at start of
   line or after whitespace") prevents `cost $5` collisions with a
   skill named `5`, but rejects `(see $build)`. Recommend: leave-of-
   token must be whitespace OR `(` OR start-of-text.
2. **Case sensitivity?** Skill discovery is case-sensitive today.
   Keep as-is unless inconsistent with `/skill`.
3. **Should `$skillname` work in steering / follow-up messages too?**
   Yes if message_expander is invoked there — confirm in implementation.

## Out of scope

- V2 inline-composition semantics (multi-mention)
- UI autocomplete for `$` (file separately if not trivial)
- Renaming or deprecating `/skill` — both stay supported
- `$ARGUMENTS` substitution changes inside skill bodies

## Notes

This is the smallest change that delivers the alternative trigger.
The "natural composition" use case (multi-mention) is the bigger prize
but is a meaningful spec/design exercise — keep it for V2.
