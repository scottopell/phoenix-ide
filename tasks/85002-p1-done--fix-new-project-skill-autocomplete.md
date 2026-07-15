# Keep project skill autocomplete stable on `/new`

## Problem

On `/new`, typing a project-specific skill such as `/phoenix-…` can show matching skills briefly and then make the autocomplete disappear after asynchronous directory/Git metadata settles.

The failure is a composition of two behaviors:

1. The new-conversation workflow initially resolves inline-reference discovery as Direct while directory validation and branch metadata are pending. Once the repository and branch are known, discovery changes to the managed/branch Git-tree root. `useInlineReferences` correctly keys catalogs by that root, so it replaces the initial live-working-directory catalog.
2. `ResolutionRoot::skills_view` materializes only paths that literally end in `SKILL.md` from the committed tree. Phoenix's project skills are exposed through tracked symlinked directories such as `.agents/skills/phoenix-development -> ../../skills/phoenix-development`. A fresh Git worktree resolves those symlinks and discovers the skills, but the synthetic Git-tree view ignores the symlink entries and therefore omits every `/phoenix-*` skill. After the root changes, fuzzy filtering `/phoenix…` finds no candidates and `InlineAutocomplete` returns `null`.

This violates the existing inline-reference contract that pre-create suggestions match the fresh worktree used for create-time expansion.

## Plan

1. Add a backend regression fixture with a committed, repository-relative symlinked skill directory and its tracked target. Assert that Git-tree skill discovery exposes the same skill as discovery in a real checkout.
2. Extend Git-tree skill materialization to represent repository-internal symlinked skill directories correctly:
   - inspect committed tree entry types rather than treating every path as a regular file;
   - resolve symlink targets lexically within the committed repository tree, without following the server filesystem or allowing traversal outside the tree;
   - materialize the target skill metadata at the logical `.claude/skills` or `.agents/skills` location expected by discovery;
   - support nested/namespaced `skills/` metadata reachable through the symlink;
   - ignore invalid, escaping, cyclic, broken, or unsupported symlinks safely.
3. Add a `/new` frontend regression test that delays Git validation/branch metadata, opens a filtered `/phoenix` autocomplete during the transition, then settles the managed branch root. Assert the final project skill catalog remains available and the active trigger is not dismissed. Verify requests use the settled mode and branch and stale responses cannot overwrite it.
4. Avoid presenting a provisional Direct catalog while the new-conversation resolution root is not yet known. Gate pre-create inline-reference discovery on directory/Git/branch readiness so the UI loads one authoritative catalog instead of flashing candidates from a root the first message will not use. Keep Direct and non-Git directories working once validation settles.
5. Run focused Rust and UI tests, then `./dev.py check`.

## Acceptance criteria

- `/phoenix-…` autocomplete on `/new` remains available after delayed Git and branch metadata settles.
- A project skill reached through a tracked repository-relative symlink is discoverable from a managed/branch Git-tree root.
- Git-tree discovery and a fresh worktree agree for the symlinked skill fixture.
- Invalid or escaping symlinks do not read outside the selected committed tree and do not fail the whole catalog.
- The autocomplete does not briefly offer live-working-directory candidates while its authoritative branch root is still loading.
- Existing Direct-mode, regular-directory skill, child-project skill, and stale-request behavior remain covered and passing.
