# Evaluate sub-agent cwd worktree containment

## Problem

Sub-agent cwd overrides may need a broader boundary than the universal “cwd must not resolve to `/`” floor. Today Work sub-agent cwd overrides from Work/Branch parents are guarded to stay inside the parent worktree, but Explore sub-agents can still be pointed elsewhere. Explore tools are read-only, but they can still consume resources or read unintended files when rooted outside the project.

This is intentionally separate from the root-cwd floor. The root floor should ship independently and universally.

## Proposed follow-up

1. Re-evaluate the intended boundary for sub-agent cwd overrides:
   - Should all sub-agents spawned by Work/Branch parents be contained to the parent worktree?
   - Should Direct parents remain unscoped except for the universal root floor?
   - Should Explore sub-agents be allowed to inspect sibling repositories or only the parent project?

2. Update specs first if the boundary changes:
   - `specs/subagents/subagents.allium`
   - `specs/subagents/executive.md`

3. Implement the chosen containment rule with canonicalized path checks and regression tests.

## Acceptance criteria

- The intended sub-agent cwd containment policy is explicit in specs.
- Implementation and tests match that policy.
- This follow-up does not change the universal root-cwd floor semantics.
