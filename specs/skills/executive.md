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

Additional turn-boundary requirements and their qualification are described below.

## Cross-Spec References

- `specs/inline-references/` -- REQ-IR-002 and REQ-IR-003 define the user-facing
  `/skill` trigger. This spec defines the internal delivery mechanism.
- `specs/keyboard-interaction/` -- REQ-KB-006 help panel lists `/` as a skill
  trigger.


## Turn-stability implementation and verification

Ordinary runtime dispatch retains a combined guidance-path/content and skill-metadata
context, separate from live permissions and invocation discovery. It refreshes after
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
These tests were qualified by hosted CI rather than local heavy builds. Runtime
authorization and Coordinator authentication paths remain unchanged.

This checkout has no automatic goal-continuation producer. Its required refresh
contract is specified, but an actual automatic-goal boundary regression remains
an integration gap, not a test pass. The broader project-guidance snapshot and
explicit refresh work tracked in task 36009 is not fulfilled by the turn-local capture
change.

## Repository guidance turn stability

REQ-SK-010 extends the ordinary live-turn snapshot to ordered guidance paths and contents. It reuses the committed boundaries from REQ-SK-009; reconstructed executors discover afresh. Skill invocation remains live, Coordinator construction and MCP intra-turn availability remain unchanged. Focused builder and model/tool-loop regressions cover guidance edits and next-turn refresh. Merged-source qualification is recorded below. This does not deliver task36009's persisted bundle/manual-refresh design.

Lifecycle/projection qualification scope: in-process ordinary retries retain the live capture. Summary uses its separate CompactionPolicy system prompt and does not inject the capture into its system block. Restart, including interrupted-turn recovery, and Continue successor reconstruction intentionally discover current files. These are accepted adoption boundaries, not unresolved limitations or a requirement to persist old snapshots. Provider projection keeps existing Anthropic system, Responses instructions, Codex Lite developer (older Codex instructions), and Chat system roles. Existing provider-tool-policy fixtures cover retained exchanges/current eligibility; this increment does not change MCP advertisement timing. Automatic Goals producer integration belongs to its owner and is not claimed by these tests.

## Accepted rediscovery contract and source qualification

The primary user explicitly accepted restart and context-exhaustion Continue loading current repository instructions. REQ-SK-010 locks this intended behavior: only an uninterrupted live executor turn retains the capture; a new turn, restart, or Continue successor resolves current files. No snapshot persistence work is owed by that contract. Existing source initializes `ordinary_turn_context` empty in `ConversationRuntime::new` and lazily discovers through `OrdinaryTurnContext::discover` on ordinary dispatch; Summary keeps its separate CompactionPolicy.

Guidance source merged in PR #966 as `c1125c6eff420adefa819bb6ecf62565b0c1728d`; ordinary main CI [39703264225](https://github.com/scottopell/phoenix-ide/actions/runs/39703264225) passed, including actual Rust tests, compilation/codegen/fmt/musl, Clippy, E2E, UI/specs and task validation. Builder rediscovery and runtime model/tool-round tests cover edited content/path/fallback and new-input behavior. Dedicated restart/Continue edited-guidance tests are not added by this documentation clarification; current behavior is supported by constructor/dispatch source inspection and existing lifecycle fixtures, not a newly claimed targeted test. Future automatic Goals producer boundary integration/regression remains owned by Goals.

MCP advertisement remains live within turns. Disable blocks new manager admission but does not guarantee cancellation or denial of already-admitted/queued calls; the user accepted that boundary for now. It is not a remediation or release gate introduced here. Other current runtime permission and audience checks remain enforced; retained guidance or metadata never grants authority. This records the accepted boundary and does not rewrite MCP policy or broaden the stable-prefix completion claim.
