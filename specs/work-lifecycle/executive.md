# Work Lifecycle — Executive Summary

## What This Spec Covers

One explicit Close operation for a Git-backed ProductConversation: inspect data and registered WorkScope ownership, show exact blockers, stop owned resources safely, retain truthful incomplete outcomes, and permit only an explicit bounded retry under the original Close authority. Close does not mutate repository refs or pull requests and does not create recovery artifacts.

## Current Contract

- Native `close_conversation` is the single idempotent Close interface. Repeated, concurrent, replayed, and lost-response calls observe the same retained operation; they do not implicitly retry it.
- The inspection and user-facing blocker projection come from one decision and identify exact files/commits plus the operational writer, permission, layout, error, or deadline reason. Relevant ownership and permissions are rechecked at mutation boundaries; a snapshot is not a guarantee of later success.
- Staged, unstaged, untracked, ignored (including build), submodule, and at-risk detached-commit content are blockers until separate owner-authorized cleanup or preservation resolves them and a fresh Close inspection confirms that result. `.gitignore` is not permission to discard. Close provides no discard override or cleanup convenience.
- Close targets authoritative registered WorkScopes and their existing shutdowns: Bash, tmux, PTY, browser, original worktree, and independent WorkScope retirement. It does not infer additional ownership from paths or metadata.
- The first failure stops further effects. A definitely stopped conversation may enter History only with confirmed shutdown and conspicuously retained incomplete blockers plus explicit Retry/Stop choices. Uncertain shutdown before any positive shutdown/History commit remains Open with `CloseIncomplete`; later uncertainty preserves prior History, positive shutdown evidence, and truthful cleanup attention. Retry is a new bounded run with an immutable typed plan and receipt committed together; Stop preserves unresolved data and ends Close effects.
- No automatic metadata-only completion successor, in-Close discard/salvage/autorepair, or disposable-state classifier is part of this contract.

## Implementation and Qualification Status

**This is a narrowed contract, not a claim that end-to-end delivery is complete.** A bounded physical WorkScope-retirement milestone is proven at exact source `6064908e3` / run `auto45205202022`: the real production detector and public physical/admin path exercised original evidence, independent WorkScope ownership, Archived/History/replay, and `nobody` capabilities; actor result was 48 passed / 0 failed; both Stop cases passed; the Linux exact-custody regression and formatting passed. These results qualify that milestone only. They do not establish that every requirement in this spec, every platform, or the complete public Close delivery is implemented or accepted.

Implementation and qualification still need to be reconciled against this bounded contract. Do not carry the superseded automatic completion-successor policy, retry budget, or disposable-classifier obligations forward as acceptance criteria. Exact typed-plan admission, positive zero-effect provenance, independent scope outcomes, and atomic Stop/source-fence terminalization remain required for explicit retries. No production cleanup, database repair, merge, or deployment authority is granted by this document.

## Acceptance Status

| Area | Contract | Status |
|---|---|---|
| Native Close identity | One idempotent `close_conversation`, exact retained operation and target | Contract revised; full implementation acceptance pending |
| Loss and blocker inspection | Exact content/commit blockers, including ignored/build content; no `.gitignore` permission | Contract revised; complete behavior acceptance pending |
| Scope and shutdown | Exact registered WorkScopes and existing Bash/tmux/PTY/browser/worktree/scope shutdowns | Physical milestone above passed; complete delivery/platform acceptance pending |
| Failure and lifecycle | Stop at first failure; new History needs confirmed shutdown and visible retained blockers; uncertainty preserves prior History or stays Open | Contract revised; full lifecycle acceptance pending |
| Explicit retry / Stop | Fresh bounded retry under original authority; Stop preserves unresolved data | Contract revised; full native public-path acceptance pending |
| Refs, PRs, recovery | No ref/PR mutation or automatic recovery artifacts | Required; end-to-end acceptance pending |

## Required Behavioral Acceptance

These are requirements, not additional tests reported as passing here:

| Scenario | Required result |
|---|---|
| Repeated/concurrent/lost-response native Close | Same exact retained request/attempt/run/result; no new topology and no implicit retry |
| Staged, unstaged, untracked, ignored/build, submodule, or detached content | Exact blocker evidence; no discard based on metadata or incomplete observation |
| Active writer, permission/layout/error/deadline failure | One blocker decision and projection naming exact target and operational cause; retain data |
| Mutation after inspection | Recheck at the mutation boundary; newly observed failure is a concrete blocker, not a claimed snapshot guarantee |
| Registered owned resources | Only exact WorkScope registrations and existing shutdown authorities are targeted |
| First failure | Stop remaining effects; retain completed steps, exact failure and residual state |
| Confirmed shutdown with incomplete cleanup | History may be shown only with conspicuous incomplete retained blockers and explicit Retry/Stop |
| Uncertain shutdown | Before positive shutdown/History commit, stay Open with `CloseIncomplete`; afterward preserve History, prior positive evidence, and truthful cleanup attention |
| Explicit retry | Atomically bind immutable typed plan or positive verified-completion provenance, successor and receipt; dispatch only the run-bound plan; no completed-effect replay or target expansion |
| Scope-only retry | Independent ordered scope outcome counts toward completeness; do not repeat tmux/worktree success or reconstruct process-epoch stop permits |
| Zero-effect explicit retry | Positive retained proof of every original resource/scope success and owned shutdown; empty or unknown plans never suffice |
| Stop/pause/cancellation/source loss | Fence effects and atomically retain terminal outcome, exact failure/residuals, shutdown-based lifecycle/attention and failure outbox; no later-callback dependency; History event only on actual transition |
| Mutation inspection/storage failure | Preserve target and fail exact run; unavailable terminal storage closes process authority and reports failure, never silent Running |
| Refs, PRs, recovery artifacts | No repository-ref/PR mutation and no automatic recovery artifact |

## Normative Authority and Follow-up

`requirements.md` and `work-lifecycle.allium` are normative for this slice and are revised together with the coupled Bedrock contracts and ADR-283. The Allium blocker decision binds an immutable complete per-scope inspection relation, including clear scopes, so execution and projection consume the same exact evidence. Independent source-only cross-spec review found no remaining blocking contradiction. Hosted Allium and artifact-shape validation is required; this is not a parser-pass claim. This executive summary does not override normative authorities.

The first implementation slice rejects new native automatic completion-successor admissions before writes while preserving exact historical receipt replay and finalization. Its focused tests cover rejection without mutation and seeded historical replay, concurrency, source revocation and cancellation; hosted execution is pending. No runtime automatic loop was removed: the affected DB method had no non-test runtime caller. Ignored/build blocker coverage, removal of in-Close discard paths, shared operational-blocker projection, and broader public/platform acceptance remain incomplete. No local scripts, builds, tests, formatter, SQL, or native experiment were run; local validation is source review and `git diff --check`.
