# Remediate PR #727 staging ownership conflict and Rust CI failure

## Observed journey

- At PR #727 exact immutable head `8c17c56d6bb4b3bbc7747e7767715de2cd3a0ee7`, cleanup of an unpublished ProductConversation staging worktree can remove a path occupied by a user-owned worktree when its repository root and HEAD OID happen to match the recorded staging metadata.
- GitHub reports PR #727 base `66c119ba9b55aa28f33f816b4a8b7aaf35d8c7e6` (`main`) and head `8c17c56d6bb4b3bbc7747e7767715de2cd3a0ee7` on `task-92009-directory-first-product-creation`. The exact-head `check (rust)` job failed in `./dev.py check (rust,cargo-fmt)`; the other reported lanes succeeded.
- This remediation must be developed from the immutable head in an isolated WorkScope and pushed only to a separate remediation branch. It must never update, force, or merge the PR #727 branch.

## Verified findings

- REQ-CCR-005 in `specs/conversation-creation/requirements.md` requires cleanup to remove only resources whose durable ownership still belongs to that cleanup operation and to return an explicit non-destructive ambiguous outcome when exact ownership cannot be proven. ADR-039 likewise makes cleanup ownership-bound and non-destructive under ambiguity.
- `cleanup_unpublished_product_staging_path` in `crates/phoenix-ide/src/runtime/creation_worker.rs` currently checks path existence, repository membership, top-level path equality, and exact HEAD OID, then executes `git worktree remove --force`. Those checks prove equivalence, not durable ownership; a replacement occupant at the same path and OID remains representable and can be deleted.
- The existing cleanup result is nested `Result<Result<bool, String>, JoinError>` and represents ambiguity as `false`; it does not structurally distinguish a typed ownership conflict from cleaned, absent, or operational failure outcomes.
- Existing tests cover deletion visibility/tombstone flow and cleanup lifecycle, but no focused regression test proves that an unowned replacement occupant survives cleanup.
- GitHub check-run evidence identifies only `check (rust)` as failed. Anonymous Actions access exposes the failing step but not the underlying log. A local diagnostic attempt was blocked before execution by Explore sandbox denial on the host uv cache, so exact failure diagnosis remains an implementation step in the approved WorkScope.
- Existing tasks 40012 and 40013 are broader directory-first parent/sibling efforts and do not match this isolated two-defect remediation slice; no matching focused task exists at this base.

## Interaction map

- Durable product-creation job/reservation metadata → creation cleanup claim/fence → `cleanup_unpublished_product_staging_path` → Git worktree occupant → cleanup release/tombstone persistence.
- A cleanup may proceed after retries or process recovery, so path plus OID cannot stand in for request-bound ownership. A stale cleanup must preserve a replacement occupant and persist/return a typed ownership-conflict outcome.
- Exact-head CI: GitHub `check (rust)` → `./dev.py check (rust,cargo-fmt)` → Rust tests/build/codegen guards. Reproduce under the approved non-sandbox WorkScope and fix only the exact failing cause.

## Bounded implementation scope

1. Starting from exact commit `8c17c56d6bb4b3bbc7747e7767715de2cd3a0ee7`, create/use an isolated WorkScope and a separate remediation branch (suggested `remediation/pr727-ownership-conflict-rust-ci`). Do not modify or push `task-92009-directory-first-product-creation`; do not merge.
2. Replace the cleanup path's equivalence-only deletion decision with request-bound durable ownership verification at the final deletion boundary. Preserve any existing but unowned/replaced occupant, and return a typed ownership-conflict outcome satisfying REQ-CCR-005. Thread that typed outcome through reconciliation so ambiguity is durably recorded without releasing the reservation or deleting the tombstone as if cleanup succeeded.
3. Add focused Rust regression coverage for at least:
   - the recorded owned staging occupant is removable;
   - an absent owned staging resource reconciles safely;
   - a replacement/unowned occupant at the same path and exact OID is preserved and yields the typed ownership conflict;
   - the conflict reaches the durable ambiguous-cleanup state and does not release ownership metadata prematurely.
4. Reproduce the exact-head Rust CI command/failure, identify its concrete cause, and apply the smallest in-scope correction. Do not absorb unrelated CI cleanup.
5. Run evidence in this order: focused tests; full `./dev.py check`; isolated `phoenix-adversarial-review` on the complete exact range `66c119ba9b55aa28f33f816b4a8b7aaf35d8c7e6..candidate_exact_head`; remediate every valid finding; rerun affected focused tests and full check.
6. Commit the completed candidate and push that immutable SHA only to the separate remediation branch. Never push the PR #727 branch.
7. After push, require exact-candidate CI success, exact-head Codex confirmation, and inspection of all paginated PR review threads. Zero unresolved threads is the completion gate. CI/Codex are confirmation after local/adversarial review, not the first review.
8. Track only this task: transition it to `in-progress` on approval/work start and to `done` only after every post-push gate passes.

## Acceptance evidence

- The user-owned replacement staging occupant still exists byte-for-byte after the cleanup attempt, and the caller receives/matches a typed ownership conflict rather than a boolean or generic string.
- Durable cleanup state remains explicit and non-destructive after conflict; no reservation/tombstone is falsely finalized.
- Exact Rust CI failure is named with root cause and corrected by focused evidence.
- Focused tests and full `./dev.py check` pass before review and again after any remediation.
- `phoenix-adversarial-review` covers the complete PR base-to-candidate range and all valid findings are resolved before push.
- The pushed remediation branch points to the reported immutable candidate SHA; PR #727 branch/head is unchanged by this work.
- CI and Codex both identify the exact candidate SHA; every paginated review thread is inspected and unresolved count is zero.

## Risks and non-goals

- Risk: ownership proof that is too weak still deletes replacements; proof that is too broad can strand genuinely owned staging. Tests must distinguish identity from equivalence and fail closed.
- Risk: changing the cleanup outcome without threading it through persistence can silently collapse conflict into success or generic failure.
- Non-goals: the remaining broad task 40013 reservation/creation work, ProductConversation UI behavior, close/member lifecycle, PR #727 branch changes, merge activity, or unrelated CI modernization.
