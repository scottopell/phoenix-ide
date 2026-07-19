---
name: phoenix-development
description: Implement, debug, test, and review changes in Phoenix IDE. Use for Phoenix coding tasks, repository workflow, validation, codegen, commits, or choosing the right specialized Phoenix skill. Start here for Work-mode tasks; use phoenix-explore first when the problem is still ambiguous.
---

# Phoenix Development

Build the smallest correct change from a verified failure model. Repository guidance and normative specs outrank this skill.

## The loop

1. **Orient.** Verify the worktree and `git status`. Locate the owning crate/UI component, nearby tests, and feature spec. Search before reading large files.
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

## Choose the validation that proves the claim

| Change | Minimum evidence before broad check |
|---|---|
| Rust logic/state transition | Focused unit/property/integration test |
| React behavior | Focused Vitest test; browser journey when layout, focus, timing, or interaction matters |
| SSE wire shape | Rust parity tests, `./dev.py codegen`, TypeScript/schema checks |
| Persistence/migration | Schema/migration tests plus old-row/crash-recovery path |
| Tool/provider boundary | Tool spec, focused adapter test, capability-gap logging |
| Deployment/lifecycle | Unit tests plus a disposable end-to-end harness; never experiment on live production |
| Production-only behavior | Narrow TraceQL/log inspection first; fetch full traces only after finding trace IDs |

A red broad check is evidence to classify, not permission to ignore it or to fix unrelated code blindly: **introduced**, **exposed**, **unrelated blocking**, or **unrelated non-blocking**. Record anything not fixed.

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
