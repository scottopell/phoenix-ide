---
created: 2026-05-03
priority: p2
status: ready
artifact: src/system_prompt.rs
---

<!--
ID 27102 chosen above 27101. Created without `taskmd new` since the binary
isn't installed; run `./dev.py tasks fix` if reallocation needed.
-->

# Skill usage guidance + `{SKILL_DIR}` substitution

## Problem

Phoenix's skill catalog tells the model only:

> "The following skills are available. Invoke them with the `skill` tool
> (e.g. skill(skill_name=\"build\")). Do not cat SKILL.md files directly."

(`src/system_prompt.rs:402-403`)

That's the entire usage instruction. Compare to:

- **Pi** (`packages/coding-agent/src/core/skills.ts:347-353`): tells the
  model "Use the read tool to load a skill's file when the task matches
  its description. When a skill file references a relative path, resolve
  it against the skill directory (parent of SKILL.md / dirname of the
  path) and use that absolute path in tool commands."
- **Codex** (`codex-rs/core-skills/src/render.rs:27-43`): ~40 lines
  covering progressive disclosure (open SKILL.md, read only enough),
  relative-path resolution against the skill dir, conventions for
  `references/` (deep dives, load only what's needed), `scripts/`
  (prefer running over re-typing), `assets/` (reuse), coordination
  between multiple skills, and safety/fallback when a skill can't be
  applied.

Phoenix gives the model none of this guidance, even though shipped
skills depend on the conventions:

- `agent-browser/`, `dogfood/`, `allium/` use `references/` subdirs
- `agent-browser/`, `dogfood/` use `templates/` subdirs
- `vercel-react-best-practices/` uses `rules/` subdir

The model has to infer the convention from "Base directory for this
skill: …" prepended to the body — there's no instruction that says
"resolve relative paths in this body against that directory" or
"`references/` are progressive deep dives, only load what you need".

### Concrete latent bug: `{SKILL_DIR}` placeholder isn't substituted

`.agents/skills/dogfood/SKILL.md:47`:

```
cp {SKILL_DIR}/templates/dogfood-report-template.md {OUTPUT_DIR}/report.md
```

`invoke_skill()` (`src/skills.rs:62`) substitutes only `$ARGUMENTS`,
`$1`, and `$ARGUMENTS[N]`. `{SKILL_DIR}` and `{OUTPUT_DIR}` pass
through verbatim. The model has to guess the substitution from the
"Base directory" line we prepend. Sometimes works, sometimes doesn't.

This is the only `{SKILL_DIR}` use across all shipped skills, but the
contract is unclear: is `{SKILL_DIR}` a documented placeholder phoenix
should support, or skill-author shorthand the model is expected to
resolve manually? Pick one and make it true.

## Goal

The model has enough guidance — in the catalog block AND in the
material returned by `invoke_skill()` — to use skills the way they
were designed: progressive disclosure, relative paths resolved
against the skill directory, conventional subdirs reused rather than
ignored.

## Two parts (can be done together or in sequence)

### Part A — Catalog usage guidance (system_prompt.rs)

Extend the `<available_skills>` block in `system_prompt.rs:399-413`
with a tight 5-8 line "How to use skills" section before listing the
skills. Keep it shorter than Codex's 40 lines (we don't have token
budget enforcement yet — see "Notes"). Cover:

- Invoke via the `skill` tool, not by cat-ing SKILL.md (already
  there; preserve)
- Resolve relative paths in skill bodies against the path printed
  next to each skill name in the catalog (or the "Base directory"
  line in the body — pick the canonical reference)
- `references/`, `templates/`, `rules/`, `scripts/`, `assets/`
  subdirs are progressive — load specific files only when needed,
  don't bulk-load entire dirs
- If a skill `scripts/` directory exists, prefer running scripts
  over reproducing their logic
- Coordinate when multiple skills apply: pick the minimal set, name
  which you're using and why

Keep the prose model-agnostic — don't bake assumptions about which
provider is in use.

### Part B — `{SKILL_DIR}` substitution OR removal (skills.rs)

Two viable directions; pick one:

#### Option B1: Implement `{SKILL_DIR}` substitution in `invoke_skill()`

Extend `substitute_arguments` (or a new `substitute_placeholders` step)
to replace `{SKILL_DIR}` with `skill.path.parent()` (the same value
already prepended as "Base directory"). Document the placeholder in
the catalog guidance from Part A. Pro: skills become portable across
checkout paths without the model having to do the substitution. Con:
adds a new contract to maintain.

#### Option B2: Remove `{SKILL_DIR}` from shipped skills, document explicit pattern

Rewrite `dogfood/SKILL.md:47` to use the absolute path (which the
model can compose from the "Base directory" line plus a relative
suffix), or to call a script that already knows its own location.
Add a one-liner to Part A's guidance: "Skill bodies do NOT have
`{SKILL_DIR}` style template substitution. Resolve relative paths
yourself using the path next to the skill name." Pro: contract
stays minimal, no surprise placeholders. Con: skill authors lose a
convenience, model does the path math.

Recommend B1 for "pit of success" alignment — substitute it for them
and the model can't get it wrong. The substitution is one line in
`substitute_arguments` and one bullet in the catalog guidance.

## Acceptance criteria

- [ ] `<available_skills>` block in `system_prompt.rs` includes
      a "How to use skills" prose section covering relative-path
      resolution, subdir conventions (references/templates/scripts/
      assets/rules), progressive disclosure, and multi-skill
      coordination
- [ ] If Option B1: `invoke_skill()` substitutes `{SKILL_DIR}` to
      the absolute skill directory; covered by a unit test in
      `src/skills.rs` (alongside the existing `$ARGUMENTS` tests)
- [ ] If Option B1: `dogfood/SKILL.md:47` works as written without
      the model having to resolve the placeholder
- [ ] If Option B2: `dogfood/SKILL.md:47` is rewritten and the
      catalog guidance explicitly notes no `{X}` substitution
- [ ] Spec update in `specs/skills/` capturing whichever path is
      chosen (B1 or B2) so future skill authors know the contract
- [ ] Manual smoke test: invoke `dogfood`, verify the model
      produces a runnable `cp` command with a real path

## Out of scope (potential follow-ups, not blockers)

- **Token budget for the catalog** (Codex enforces ~8KB / 2% of
  context with description truncation). Worth filing if the catalog
  ever grows enough to crowd context. Today it's small.
- **Per-skill policy fields** (Codex has `allow_implicit_invocation`,
  `products`). File separately if user-invoked-only skills become
  a real need.
- **Skill roots aliasing** for compact catalog paths. Useful only
  with many skills.

## Notes

- The dual-source-of-truth concern: relative-path resolution
  guidance lives in TWO places — the catalog (Part A) and the
  per-body "Base directory" line in `invoke_skill()`. Keep them
  consistent. Recommendation: catalog states the rule once;
  body keeps the "Base directory" line as a per-invocation
  reminder. Don't duplicate the prose.
- This task is independent of 27101 ($-mention syntax) and 27100
  (OpenAI tool search). All three can land in any order.
- Phoenix wraps skill body delivery behind the `skill` tool, which
  amplifies the guidance gap relative to Pi/Codex (which return
  bodies via the standard `read` tool result and rely on the model's
  general-purpose file-handling instincts). The wrap is a feature
  — argument substitution and base-dir prepending are real value —
  but it makes explicit guidance more important.
