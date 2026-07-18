# Show the current worktree checkout in Workspace Diff

## Problem

The Workspace Diff viewer explains what changed relative to the conversation base, but it does not answer the more immediate Git question: **what is this worktree checked out on right now?**

Users need to distinguish:

- a named local branch with a matching remote-tracking branch;
- commits on that branch that have not been pushed;
- a branch that is behind or diverged from its remote-tracking branch;
- a local branch with no known remote copy; and
- a valid detached HEAD, including a checkout such as `git checkout --detach origin/main`.

The stored conversation branch/base cannot answer this reliably because the checkout may have changed after conversation creation. The live worktree must be observed when the diff payload is built.

## Product behavior

Add a compact **Checkout** status panel near the top of Workspace Diff, before the commit list. It should communicate checkout identity first and remote relationship second.

Examples:

- `BRANCH  feature/foo`
  `REMOTE  origin/feature/foo · 2 to push`
- `BRANCH  feature/foo`
  `REMOTE  origin/feature/foo · up to date`
- `BRANCH  feature/foo`
  `REMOTE  origin/feature/foo · 2 to push · 1 behind`
- `BRANCH  feature/foo`
  `REMOTE  No known remote branch`
- `DETACHED HEAD  a1b2c3d`
  `POINTS TO  origin/main`

Use inline status color/symbols consistently with Phoenix's information-density guidance: neutral for identity, green for up to date, yellow for unpublished/ahead/behind, and red only for genuine divergence or observation failure. Include accessible text; color must not carry the meaning alone.

Detached HEAD is a first-class valid state, not an error. Git does not retain the source ref after `checkout --detach`; therefore the UI must not claim the worktree is “on origin/main.” It may say the detached commit **points to** `origin/main` (or another exact ref) only when that ref currently resolves to the same commit. Always show the abbreviated HEAD OID so the identity remains truthful if refs move.

Remote status is a read-only snapshot from local remote-tracking refs. Opening the viewer must not fetch or perform other network I/O. Label or tooltip this as based on the last fetched state so “no known remote branch” is not presented as authoritative knowledge of the server.

## Data model

Extend the conversation diff payload with a typed checkout-status sum rather than nullable, parallel fields. The wire model must make these states structurally distinct:

- named branch: branch name, HEAD OID, and typed remote relationship;
- detached HEAD: HEAD OID and zero or more exact local/remote refs pointing at it;
- unborn HEAD;
- unavailable observation with a display-safe reason.

For a named branch, model the remote relationship explicitly:

- tracked upstream with ref and ahead/behind counts;
- a conventional matching remote-tracking ref found without configured upstream (for example `origin/feature/foo`), also with ahead/behind counts and identified as not configured for tracking;
- no known remote-tracking branch;
- unavailable relationship/counts.

Do not flatten these into optional `branch`, `upstream`, `ahead`, and `behind` fields that permit contradictory combinations. Do not conflate the workspace comparator (`origin/main`) with the checked-out branch's remote copy (`origin/feature/foo`): they answer different questions.

Ahead/behind semantics use `HEAD...<remote-tracking-ref>`:

- ahead > 0, behind = 0: commits to push;
- ahead = 0, behind > 0: local checkout is behind;
- both > 0: diverged;
- both zero: up to date.

Resolve a configured upstream first (`@{upstream}`). If none exists, recognize a matching `origin/<branch>` remote-tracking ref as a known copy while clearly distinguishing it from a configured upstream. Do not infer push state from the conversation base branch.

Reuse `phoenix_core::git::observe_local_git_head` / `LocalGitHeadObservation` for live checkout identity rather than creating a second branch-observation implementation. Add focused Git helpers for upstream resolution, exact refs, and ahead/behind parsing where they can be shared and tested independently.

## Implementation plan

1. Add a timeless requirement under the `projects` specification for live worktree checkout identity in the Workspace Diff surface, including named, detached, unborn, unavailable, remote-tracking freshness, and no-network behavior. Update the projects executive coverage table/current-reality text. No Allium addition is needed for this read-only observation/UI behavior.
2. Introduce typed Rust wire enums/structs in `crates/phoenix-ide/src/api/types.rs` for checkout and named-branch remote state.
3. In `get_conversation_diff` (`crates/phoenix-ide/src/api/git_handlers.rs`), observe the live worktree and calculate the local remote-tracking relationship while already inside `spawn_blocking`. Keep `capture_branch_diff` and its comparator behavior unchanged.
4. Return and display the same live checkout status from both diff endpoints so the shared response and `DiffView` have one total shape. Workspace Diff is the motivating and required surface; showing the same context in PR Diff avoids a parallel partial representation.
5. Extend `ConversationDiffResponse` in `ui/src/api.ts` with an equivalent discriminated union and thread it through `ConversationDiffViewer` into `DiffView`.
6. Add a compact, responsive checkout panel before `CommitLogSection`. Colocate new viewer-specific CSS with the owning diff-view component per CSS ownership guidance. Ensure the panel remains visible in an otherwise-empty diff, because “what is checked out?” is useful even when there are no changes.
7. Update API/component fixtures and tests, then run codegen only if the chosen Rust wire types participate in existing `ts-rs` generation.

## Verification

Backend tests must cover real temporary repositories/worktrees for:

- named branch with configured upstream: up to date, ahead, behind, and diverged;
- matching `origin/<branch>` exists but is not configured as upstream;
- no matching remote-tracking branch;
- configured upstream on a remote other than `origin`;
- detached HEAD at a commit also pointed to by `origin/main`;
- detached HEAD with no exact named ref;
- multiple exact refs pointing at a detached commit, with deterministic ordering and bounded output;
- unborn HEAD;
- observation/upstream-count failure represented as typed unavailable state;
- no fetch/network command is issued by the diff request;
- workspace comparator remains independent from checkout remote status.

Frontend tests must cover:

- branch identity and up-to-date state;
- `N to push`, behind, and diverged labels;
- no known remote branch and last-fetched qualification;
- detached HEAD with and without exact pointing refs;
- unborn/unavailable rendering;
- accessibility without relying on color;
- checkout status shown when commit and file diffs are empty;
- responsive wrapping for long branch/ref names.

Run focused Rust and UI tests during development, then `./dev.py check`. Perform a Ladle/browser visual pass at desktop and mobile widths with long refs and all status classes.

## Acceptance criteria

- Workspace Diff clearly identifies the worktree's **live** checkout, not merely the conversation's stored branch.
- Named branches show their configured upstream or known matching remote-tracking copy and truthful ahead/behind state.
- Users can tell when commits remain to be pushed.
- A branch with no locally known remote copy is communicated without claiming authoritative live-server state.
- Detached HEAD is clearly and neutrally represented by OID; exact pointing refs are supporting context, never misrepresented as checkout provenance.
- Checkout status remains visible when the workspace has no changes.
- Opening Workspace Diff remains read-only, bounded, and network-free.
- Invalid combinations of checkout/remote state are unrepresentable in the backend and frontend types.
- Existing comparator, committed/uncommitted diff, truncation, PR-diff, and review-note behavior does not regress.
