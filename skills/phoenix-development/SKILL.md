---
name: phoenix-development
description: Implement, debug, test, and review changes in Phoenix IDE. Use for Phoenix coding tasks, repository workflow, validation, codegen, commits, or choosing the right specialized Phoenix skill. Start here for Work-mode tasks; use phoenix-explore first when the problem is still ambiguous.
---

# Phoenix Development

Build the smallest correct change from a verified failure model. Repository guidance and normative specs outrank this skill.

## The loop

1. **Orient.** Verify the worktree and `git status`. Read the generated body of GitHub Issue #806 (the delivery roadmap) for current delivery context. Locate the owning crate/UI component, nearby tests, and feature spec. Search before reading large files.
2. **Model.** State the user-visible failure, violated invariant, and boundary that should own the fix. Do not edit from a symptom alone.
3. **Trace.** Follow the path end to end where relevant: UI/state → API/SSE → runtime/state machine → persistence/provider/tool. Read existing tests and history around the seam.
4. **Regress.** Add or identify the narrowest test that can falsify the hypothesis. Reproduce first when practical.
5. **Fix structurally.** Prefer a type, constructor, schema constraint, or shared boundary over repeated call-site discipline. Keep local fixes local when no shared policy exists.
6. **Validate in widening rings.** Focused test → owning crate/UI checks → generated artifacts/spec checks → `./dev.py check`. For user-visible behavior, also exercise the real journey.
7. **Review and integrate.** Inspect the diff, classify failures, commit logical units, absorb review, and revalidate semantic conflict seams after rebases.

Read [references/development-loop.md](references/development-loop.md) for commands, source-of-truth routing, and recovery paths.

## Non-negotiable decisions

- Use `./dev.py`; never run the Phoenix server with `cargo run`.
- Before changing specified behavior, read `requirements.md` and any `.allium`. Use `executive.md` for current status and ADRs for historical rationale.
- If Rust SSE wire types change, run `./dev.py codegen`; never hand-edit `ui/src/generated/`.
- Persist addressable structure in columns/rows. Child collections are tables, not JSON arrays. Earned polymorphic blobs must serialize losslessly.
- Make invalid states unrepresentable. Do not add parallel representations of one semantic value or silently omit unsupported data.
- Treat worktrees as owned environments: never move a branch checked out in another worktree.
- Treat server paths as server-local handles. Host OS actions require the structural same-host gate.
- Use `foo.rs` + `foo/`, never `foo/mod.rs`.
- Use `taskmd` for durable task operations; do not hand-create task filenames.
- Commit completed units on the owned branch. Do not leave finished work as a long-lived dirty tree.

These are compact reminders, not replacements for `AGENTS.md` or normative specs.

## Type and authority refactors

When a change establishes an authority, lifecycle, or shutdown boundary, make the boundary compile-time visible before migrating behavior:

1. Introduce the non-optional type or exhaustive enum first and restore a green compile boundary.
2. Separate read-only discovery from independently committable mutation units. Admit each mutation at the owning orchestration layer; do not pass application-local capability types into lower-level persistence crates.
3. Use mutable reborrows for nested synchronous work and owned transfer for queued work. If the caller must continue authoritatively after a queue hop, return the same affine token through the reply rather than minting another capability.
4. Require the capability at high-level mutation, publication, filesystem, and external-dispatch sinks. A predicate or cancellation check is supporting defense, not primary enforcement.
5. Give constructors production-valid defaults. If production always installs an authority fence or lifecycle owner, do not let unit-test construction represent a runtime without one.
6. For bounded teardown, capture one absolute deadline at the first close and pass it through every nested drain. Never restart a full grace period at each layer.

Use compiler errors as the migration inventory. After each slice, run formatting plus the owning crate's all-target compile before changing the next seam.

## Recover from partial delegated edits

Treat a failed sub-agent patch as untrusted until inspected. Before continuing:

1. Record `git status` and the file-local diff; identify exactly what existed before delegation.
2. Either finish the delegated slice immediately or revert only its delta. Never leave mixed old/new enums, match arms, or enforcement paths.
3. Restore `cargo fmt` and the narrowest all-target compile before any further refactor.
4. Re-run semantic race tests after compile recovery; a compiling ownership rewrite can still shorten a capability lifetime or change which caller observes failure.

Prefer delegating bounded leaf slices with explicit file limits and validation commands. Keep ownership-sensitive integration work with one implementation owner.

## Report on the roadmap

Use the roadmap only for outcomes substantial enough that another agent or harness needs to know their owner, gate, or delivery state. Do not post routine edits, transient test failures, or leaf tasks.

1. Fetch Issue #806 by its known number; do not discover it with GitHub search. If GitHub is unavailable, state that roadmap context could not be verified.
2. Follow `specs/roadmap/requirements.md` and the examples in `specs/roadmap/executive.md`: post one `phoenix-roadmap` fenced record per comment, declare `actor.role` and `actor.harness`, and post only the kinds your role allows. Check a record with `node scripts/roadmap-issue-reducer.mjs --validate` before posting.
3. Report runtime and transcript mechanics in your own harness, not on the roadmap. The roadmap sees their effects: evidence, status, gates, and decisions.
4. After posting, re-read the Issue body. If your comment ID is at or below `phoenix-roadmap:snapshot-through` and at or above `phoenix-roadmap:ack-window-from`, it is accepted only if listed in the `phoenix-roadmap:accepted` marker and rejected if listed under "Recent rejections"; fix and re-post a rejected record. Below the window the body makes no claim.

## Reasoning about async test completion

Before waiting, identify the completion contract:

1. What work does the awaited call own?
2. What does successful return guarantee?
3. Can work continue after it returns?
4. What observable state proves the behavior?

If successful return establishes the postcondition, assert it immediately: inspect a buffered channel with `try_recv`, query an awaited database write, or read resulting state/captured calls. A second wait incorrectly implies that work remains pending.

If work intentionally outlives the call, observe the owning lifecycle boundary: a task handle, channel message, notification, lifecycle event, persisted transition, or readiness marker. The signal must be causally downstream of the behavior under test, not merely evidence that something ran.

If no completion signal exists, do not substitute sleeps or short polling. Treat that as a testability gap and expose the narrowest real lifecycle event or domain state.

A timeout is an outer liveness guard, never synchronization. Use one only around genuinely outstanding work, assume CI may be heavily CPU- and I/O-starved, and prefer virtual time when timer behavior itself is under test. The test must pass because the observable condition occurred—not because a duration elapsed.

A good async test can explain who owns pending work, what marks completion, why the observation proves the behavior, and what any remaining timeout guards. If those answers are unclear, the test likely asserts at the wrong boundary.

## Choose the validation that proves the claim

| Change | Minimum evidence before broad check |
|---|---|
| Rust logic/state transition | Focused unit/property/integration test |
| Async/concurrent behavior | Test the owning lifecycle boundary: assert established postconditions immediately, or await an explicit completion signal when work outlives the call. Use wall-clock time only as an outer liveness bound. |
| React behavior | Focused Vitest test; browser journey when layout, focus, timing, or interaction matters |
| SSE wire shape | Rust parity tests, `./dev.py codegen`, TypeScript/schema checks |
| Persistence/migration | Schema/migration tests plus old-row/crash-recovery path |
| Tool/provider boundary | Tool spec, focused adapter test, capability-gap logging |
| Deployment/lifecycle | Unit tests plus a disposable end-to-end harness; never experiment on live production |
| Production-only behavior | Narrow TraceQL/log inspection first; fetch full traces only after finding trace IDs |

A red broad check is evidence to classify, not permission to ignore it or to fix unrelated code blindly: **introduced**, **exposed**, **unrelated blocking**, or **unrelated non-blocking**. Record anything not fixed.

When a broad run fails after focused tests pass, reproduce the named test alone, then run its entire owning module. If multiple standalone fixtures fail on the same missing production dependency, fix the constructor/type invariant instead of patching fixtures one by one. Distinguish a deterministic test failure from a suite timeout caused by an earlier failure leaving companion tests blocked; resolve the first causal failure before treating the timeout as a separate defect.

## Route specialized work

| Need | Skill |
|---|---|
| Ambiguous feedback or breadth-first investigation | `phoenix-explore` |
| Create/update task state | `phoenix-task-tracking` |
| Rust implementation/review | `rust-dev` |
| React implementation/performance patterns | `vercel-react-best-practices` |
| Browser interaction | `agent-browser`; exploratory QA → `dogfood` |
| Allium behavior work | `allium:distill`, `allium:tend`, `allium:propagate`, or `allium:weed` |
| spEARS v2/spec migration | `spears`, `spears-v2-migrate` |
| Production deploy/diagnosis | `phoenix-deployment` |
| Release | `phoenix-release` |
| React performance campaign | `phoenix-perf-preflight` then the perf workflow |
| Crate extraction | `phoenix-extract-crate` |
| Ladle fixture | `phoenix-ladle-fixture` |

Invoke the specialized skill instead of duplicating its procedure here.

## Stop and re-ground when

- the named file/error/route is absent—verify repo/worktree identity;
- a patch anchor is stale or ambiguous—reread and use a wider structural anchor;
- the same command fails twice—inspect the failed assumption instead of retrying unchanged;
- code and normative spec disagree—determine which is wrong before proceeding;
- a rebase touches the fixed invariant—rerun seam-local regressions;
- you are about to say “done” without focused tests, broad-check classification, diff review, and branch-state verification.

Arguments: $ARGUMENTS
