---
created: 2026-04-07
priority: p3
status: done
artifact: src/message_expander.rs
---

# Deduplicate skill discovery filesystem traversals

## Problem

`discover_skills()` walks from the conversation's working directory up to
filesystem root, scanning `.claude/skills/` and `.agents/skills/` at every
level plus `$HOME`. This is called redundantly on the message hot path:

1. `message_expander.rs:176` -- `expand()` calls `discover_skills()` to check
   if a `/token` matches a skill name
2. `skills.rs:32` -- `invoke_skill()` calls `discover_skills()` **again** to
   find the same skill it just matched

That's two full directory traversals per message containing a `/skill-name`.

The `GET /api/conversations/{id}/skills` endpoint (autocomplete) does one
traversal per request, which is fine on its own but adds up if the frontend
polls frequently.

## Fix

1. In `expand()`: pass the already-discovered `Vec<SkillMetadata>` into
   `invoke_skill()` instead of letting it rediscover. Change `invoke_skill`
   signature to accept `&[SkillMetadata]`.

2. Consider whether the skills endpoint should cache results briefly (e.g.,
   1-2 seconds) to avoid redundant walks on rapid autocomplete keystrokes.
   Not critical -- the walk is fast for shallow directory trees -- but worth
   measuring if users report input lag.

## Done when

- [ ] `invoke_skill()` accepts pre-discovered skills instead of rediscovering
- [ ] Only one `discover_skills()` call per message expansion
