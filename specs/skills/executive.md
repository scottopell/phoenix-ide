# Skill System - Executive Summary

## Requirements Summary

Skills are reusable instruction sets stored as SKILL.md files that users invoke
by typing `/skill-name` or that the LLM invokes programmatically via the Skill
tool. The system strips YAML frontmatter before delivery, prepends the skill's
base directory path so the AI can read companion files, substitutes argument
placeholders, and delivers the result as an authoritative user-role message.
Both invocation paths call the same `invoke_skill` function for identical
content. The user path delivers as `MessageContent::Skill` (user role); the
LLM Skill tool path delivers as a tool result (pending YF616 for full
convergence via `newMessages` on `ToolOutput`).

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
| **REQ-SK-001:** Frontmatter Separation | ✅ Complete | `skills::strip_frontmatter` applied on both paths via `invoke_skill` |
| **REQ-SK-002:** Authoritative User Messages | 🔄 Partial | User path delivers as `MessageContent::Skill` (user role). LLM Skill tool still delivers as tool result (YF616 tracks convergence) |
| **REQ-SK-003:** Base Directory Context | ✅ Complete | Prepended by `invoke_skill` on both paths |
| **REQ-SK-004:** Argument Substitution | ✅ Complete | `$ARGUMENTS`, `$ARGUMENTS[N]`, `$N` positional. Named args not yet supported |
| **REQ-SK-005:** Unified Invocation | 🔄 Partial | Both paths call shared `invoke_skill` (identical content). Delivery differs: user path = `MessageContent::Skill`, tool path = `ToolOutput`. Full convergence requires `newMessages` on `ToolOutput` (YF616) |
| **REQ-SK-006:** Skill Discovery | ✅ Complete | CWD walk-up, children, `$HOME`, symlink + content dedup |
| **REQ-SK-007:** Skill Metadata in System Prompt | ✅ Complete | Catalog injected with names + descriptions |

**Progress:** 4 of 7 complete, 2 partial, 1 blocked by YF616

## Cross-Spec References

- `specs/inline-references/` -- REQ-IR-002 and REQ-IR-003 define the user-facing
  `/skill` trigger. This spec defines the internal delivery mechanism.
- `specs/keyboard-interaction/` -- REQ-KB-006 help panel lists `/` as a skill
  trigger.


## Turn-stability implementation and verification

Ordinary runtime dispatch retains a rendered metadata-only catalog, separate from
live prompt guidance, permissions, and invocation discovery. It refreshes after
successful ordinary input/state persistence (including approval feedback and
question answers), committed authoritative input, or a committed queued
start from Idle; a mid-turn queued drain and in-process retries retain it.
Reconstructed executors discover anew: no persisted snapshot/restart parity is
claimed. Boundary changes rebuild system content and may miss the provider cache;
no provider event format or capability claim was added.

REQ-SK-009 regression tests cover edit/add/remove metadata across a model/tool
loop, rejected busy input, failed state persistence retaining the prior snapshot,
next ordinary turn, durably seeded mid-turn and queued Idle drains, exact source
attribution, current body edits, shadowing and fallback, and not-found deletion.
These tests are authored but not executed locally: host admission forbids heavy
builds. Runtime authorization and Coordinator authentication paths are unchanged;
their existing regressions still require CI execution.

This checkout has no automatic goal-continuation producer. Its required refresh
contract is specified, but an actual automatic-goal boundary regression remains
an integration gap, not a test pass. The broader project-guidance snapshot and
explicit refresh work tracked in task 36009 is not fulfilled by this catalog-only
change.
