# Split no-PR WorkActions cleanup by work-change state

## Problem

The WorkActions bar currently treats every idle Work/Branch conversation with no associated PR as terminal-cleanup-ready:

- `idle + no PR found + refresh ok` derives `no_pr`
- `no_pr` makes **Clean up** the glowing primary
- `Clean up` is a single-click `mark-merged` action that deletes the worktree and, in Work mode, the managed task branch

That is correct for a clean worktree, but too aggressive when the branch/worktree has changes and no PR. In that case the work is not terminal; the user likely needs to review the diff or open a PR, not one-click cleanup.

## Desired behavior

Split the current `no_pr` case by work-change state:

| State | Primary | Clean up |
|---|---|---|
| idle + no PR + clean | **Clean up** | shown |
| idle + no PR + dirty + safely PR-ready on GitHub | **Open/Create PR on GitHub ↗** | suppressed |
| idle + no PR + dirty + not PR-ready | **View Diff** | suppressed |
| idle + no PR + work-change state unknown/loading | **View Diff** or non-destructive fallback | suppressed until known |

Keep **Abandon** available as a terminal escape hatch, but do **not** make Abandon the hero for dirty/no-PR. Do not add terminal commit/push/merge actions as the hero.

## Product rules

1. **Clean up is terminal.** It should be primary only when Phoenix can tell cleanup is a reasonable terminal action:
   - no PR + clean worktree/branch, or
   - PR merged, or
   - gh unavailable manual-cleanup fallback, preserving the existing warning behavior.
2. **Dirty no-PR work is not terminal.** Its primary action should move the user toward review or PR creation.
3. **Open PR on GitHub is only shown when honest.** Only offer a PR link when Phoenix can construct a GitHub compare/new-PR URL and the remote branch represents the local committed work closely enough that the PR would include the reviewed changes.
4. **View Diff is the safe universal dirty fallback.** If uncommitted changes, unpushed commits, unknown remote state, or non-GitHub remotes prevent an honest PR link, make View Diff the primary.
5. **No terminal automation hero.** Do not add a hero button that commits, pushes, merges, or opens a terminal to do those operations.

## Proposed implementation plan

### 1. Add a lightweight work-change summary

Add a backend endpoint or extend an existing PR/work-status response with a small summary that does not require fetching the full diff body in the action bar.

Suggested shape:

```ts
type WorkChangeSummary =
  | { kind: 'clean' }
  | {
      kind: 'dirty_pr_ready';
      create_pr_url: string;
      branch_name: string;
      base_branch: string;
    }
  | {
      kind: 'dirty_needs_review';
      reason:
        | 'uncommitted_changes'
        | 'branch_not_pushed'
        | 'local_ahead_of_remote'
        | 'remote_diverged'
        | 'non_github_remote'
        | 'unknown_remote'
        | 'unknown';
    }
  | { kind: 'loading' }
  | { kind: 'unavailable'; reason: string };
```

Backend checks should use cheap git commands:

- committed work relative to base/comparator
- uncommitted/staged/untracked work
- current branch name and base branch
- whether `origin/<branch>` exists
- whether local branch is ahead/diverged from remote
- whether origin is GitHub and can produce a compare/new-PR URL

Avoid pulling full diff text for the bar. The existing `/api/conversations/:id/diff` remains the payload for the diff viewer.

### 2. Thread the summary into WorkActions

Extend `WorkControlBarProps` and `WorkDispositionInput` with the work-change summary.

The input should be structural, not inferred from labels or notes. `deriveWorkDisposition` should not inspect raw diff strings.

### 3. Extend the disposition model

Add new disposition/verb concepts, likely:

```ts
export type BarPrimary = 'none' | 'review' | 'resolve' | 'clean_up' | 'abandon';

export type ResolveVerb =
  | { kind: 'address_feedback' }
  | { kind: 'merge_pr'; url: string; number: number }
  | { kind: 'open_pr'; url: string; number: number }
  | { kind: 'create_pr'; url: string; branchName: string };
```

For dirty/no-PR not PR-ready, make `primary: 'review'` so the existing View Diff button can glow.

For dirty/no-PR PR-ready, make `primary: 'resolve'` with `resolve.kind === 'create_pr'` and render an honest GitHub link such as `Create PR on GitHub ↗`.

### 4. Update render behavior

- Let the REVIEW zone's `View Diff` button receive `work-actions-btn--primary` when `primary === 'review'`.
- Render a new RESOLVE-zone link for `create_pr`.
- Suppress `Clean up` for dirty/no-PR dispositions.
- Keep `Abandon` available as a non-primary terminal escape hatch.
- Add inline notes for dirty no-PR states, for example:
  - `Changes found but no PR. Review the diff before cleanup.`
  - `Branch is not pushed. Review the diff, then push/open a PR.`
  - `Uncommitted changes found. Review, commit, and push before opening a PR.`

### 5. Update specs

Update `specs/work-actions-bar/`:

- `requirements.md`
  - split REQ-WAB-004 current row 8 into clean no-PR and dirty no-PR rows
  - update single-primary wording to explicitly allow REVIEW primary for dirty/no-PR
  - add/adjust requirements for honest create-PR link behavior
- `design.md`
  - update disposition table and rendering table
  - describe work-change summary input and PR-ready constraints
- `work-actions-bar.allium`
  - add work-change state to `BarInputs`
  - add dispositions/rules for no-PR clean, no-PR dirty PR-ready, and no-PR dirty needs-review
  - update invariants so `resolve != null iff primary === resolve` still holds, and `primary === review` implies View Diff is shown and glows

Keep the specs timeless: no references to this task, refactor history, or current bug wording.

### 6. Update tests

Add/update unit tests for `deriveWorkDisposition`:

- no PR + clean → `clean_up`, Clean up shown
- no PR + dirty PR-ready → `resolve/create_pr`, Clean up hidden, Abandon shown
- no PR + dirty uncommitted → `review`, Clean up hidden, Abandon shown
- no PR + dirty branch not pushed → `review`, Clean up hidden, note explains push/PR path
- no PR + work-change loading/unknown → no Clean up hero
- merged PR still → Clean up
- gh unavailable no-PR fallback preserves existing Clean up warning behavior unless dirty-state semantics intentionally override it

Add/update component tests:

- View Diff can be the single glowing primary
- Create PR link renders as an external link with ↗ and correct URL
- Clean up is absent/non-primary in dirty no-PR cases
- exactly one primary still holds across representative render cases

Add backend tests for work-change summary classification:

- clean branch
- committed changes not pushed
- committed changes pushed/up-to-date with GitHub remote
- uncommitted changes
- remote branch missing
- local ahead/diverged from remote
- non-GitHub remote

### 7. Validation

Run targeted checks first, then full project check:

```bash
./dev.py codegen   # if generated API/SSE types change
./dev.py check --lanes ui
./dev.py check --lanes rust
./dev.py check
```

If Allium is edited, also run the relevant `allium check` command for `specs/work-actions-bar/work-actions-bar.allium`.

## Acceptance criteria

- A dirty Work/Branch conversation with no PR no longer shows **Clean up** as the hero.
- Clean no-PR conversations retain the current **Clean up** behavior.
- Dirty no-PR conversations either:
  - show **Create/Open PR on GitHub ↗** when Phoenix can honestly link to PR creation for the pushed branch, or
  - show **View Diff** as the primary otherwise.
- **Abandon** remains available but is not the dirty/no-PR hero.
- The single-primary invariant still holds across REVIEW, RESOLVE, and FINISH zones.
- Specs and tests reflect the new split; no spec still requires dirty/no-PR to map to Clean up.
