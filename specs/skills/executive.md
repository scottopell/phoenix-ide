# Skill System - Executive Summary

## Requirements Summary

Skills are reusable instruction sets stored as SKILL.md files that users invoke
by typing `/skill-name` or that the LLM invokes programmatically via the Skill
tool. The system strips YAML frontmatter before delivery, prepends the skill's
base directory path so the AI can read companion files, substitutes argument
placeholders, and delivers the result as an authoritative user-role message.
Both invocation paths produce the same LLM-facing representation -- no
divergent code paths.

## Technical Summary

Skills are discovered from `.claude/skills/` and `.agents/skills/` directories
at each level from CWD to root, plus child directories and `$HOME`. A shared
`invoke_skill` function handles frontmatter stripping, base directory prepend,
and argument substitution. The result is delivered as a `MessageContent::Skill`
variant -- a user-role message marked as system-generated. The existing message
expander handles the user `/skill` path; the Skill tool handles LLM-initiated
invocation. Both converge to the same delivery function.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-SK-001:** Frontmatter Separation | 🔄 In Progress | Parsing exists; stripping not applied on user path |
| **REQ-SK-002:** Authoritative User Messages | ❌ Not Started | Currently delivered as raw text (user path) or tool result (LLM path) |
| **REQ-SK-003:** Base Directory Context | ❌ Not Started | Not prepended on either path |
| **REQ-SK-004:** Argument Substitution | ✅ Complete | $ARGUMENTS, $N, positional all working |
| **REQ-SK-005:** Unified Invocation | ❌ Not Started | Two divergent paths produce different representations |
| **REQ-SK-006:** Skill Discovery | ✅ Complete | CWD walk-up, children, $HOME, dedup |
| **REQ-SK-007:** Skill Metadata in System Prompt | ✅ Complete | Catalog injected with names + descriptions |

**Progress:** 3 of 7 complete

## Cross-Spec References

- `specs/inline-references/` -- REQ-IR-002 and REQ-IR-003 define the user-facing
  `/skill` trigger. This spec defines the internal delivery mechanism.
- `specs/keyboard-interaction/` -- REQ-KB-006 help panel lists `/` as a skill
  trigger.
