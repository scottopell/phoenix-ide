# Reduce patch anchor failures with production-driven recovery improvements

## Summary

Improve the patch tool's agent success rate using the dominant production failure trajectories as the design input. The implementation must select and validate a safe intervention for missing and non-unique anchors, preserve atomic and unambiguous editing, and make recovery measurably more effective without silently guessing where to edit.

## Production Evidence

A read-only analysis of the production Phoenix database found:

- 4,690 patch calls and 605 marked patch errors in the initial snapshot: a 12.9% raw error rate;
- 302 non-unique-anchor errors and 264 missing-anchor errors, together representing 93.6% of marked patch failures;
- duplicate errors that included matching locations were followed by a successful patch retry within ten minutes 88.5% of the time, versus 82.2% without locations;
- after the June 24 anchored-insert/replaceAll rollout, the broad adjusted patch error rate fell from 15.78% to 10.39%, but a sensitivity window excluding patch-development dogfooding estimated a smaller 12.07% to 10.41% change and was not conclusive;
- the current matcher already attempts exact, dedent, trimmed-line, and Unicode-skeleton strategies, so another permissive fuzzy matcher is not justified without evidence and safety analysis.

The data supports improving recovery, but does not yet identify one safe implementation for both error classes. The task should therefore begin by classifying representative failure-and-retry trajectories, then implement the smallest intervention supported by that evidence.

## Safety Constraints

- A patch must never mutate multiple possible sites without the agent's explicit `replaceAll` request.
- A single-site operation must never silently choose among ambiguous candidates.
- All patches in one call must continue to resolve against original content and apply atomically.
- Failure diagnostics must stay bounded and must not flood the context with source content.
- Matching behavior and recovery guidance must remain consistent across `replace`, `insert_before`, and `insert_after`.
- Production data must be queried read-only, summarized by default, and not copied into fixtures or committed artifacts.

## Plan

### 1. Establish a reproducible baseline

- Re-run the production analysis immediately before implementation using `is_error=true` results joined to patch calls by `conversation_id` and `tool_use_id`.
- Record calls, errors, missing-anchor errors, non-unique-anchor errors, retry success, model mix, classifier coverage, and the exact observation window.
- Separate infrastructure/session failures and ordinary command exit codes from patch misuse.
- Add deterministic repository fixtures that reproduce the *shapes* of observed failures without containing production paths, prompts, or source text.

### 2. Analyze failure trajectories

Inspect a bounded, redacted sample of missing/non-unique failures together with the preceding patch input and immediate recovery actions. Classify at least:

- stale anchor after an earlier edit;
- truncated or over-broad anchor;
- whitespace/line-ending mismatch not recovered by the existing cascade;
- multiple identical blocks where surrounding context can disambiguate;
- multi-patch simultaneous-resolution misunderstanding;
- an operation better expressed as `insert_before`, `insert_after`, or explicit `replaceAll`;
- failures where the existing error already gives sufficient recovery information.

Quantify class coverage before selecting an intervention. Do not add matching heuristics for a rare class while leaving the dominant class unaddressed.

### 3. Implement a safe recovery improvement

Choose the smallest evidence-supported change. Candidate directions include, but are not limited to:

- richer bounded diagnostics for `OldTextNotFound`, such as line-numbered near-match candidates and a concise mismatch explanation;
- duplicate diagnostics that provide copy-ready disambiguating context rather than snippets that still match multiple sites;
- structured error variants carrying typed recovery data, rendered consistently into LLM-visible text;
- schema/description changes that steer agents toward anchored insertion, `replaceAll`, or sequential calls when those are the correct operations;
- deterministic candidate ranking used only for diagnostics, never for silently applying an ambiguous edit.

If trajectory analysis shows more than one intervention is necessary, keep the implementation bounded to the smallest coherent set and record unrelated opportunities rather than expanding into a matcher rewrite.

### 4. Specify and test the behavior

- Update `specs/patch/requirements.md`, `specs/patch/patch.allium`, and `specs/patch/executive.md` for the selected standing behavior; keep requirements and Allium timeless and put rationale in an ADR if a durable design decision warrants one.
- Add unit/property tests for exact, dedent, trimmed, skeleton, missing, duplicate, Unicode, long-line, large-file, and bounded-output cases affected by the change.
- Add trajectory-style tests showing that diagnostics provide enough information for the next request to construct a unique valid anchor.
- Preserve existing ambiguity rejection, original-content planning, atomic rollback, clipboard semantics, and output bounds.

### 5. Evaluate

- Run focused patch tests and `./dev.py check`.
- Validate diagnostics against the held-out deterministic failure shapes.
- Define a post-deployment review boundary using the exact startup `git_sha` marker. If retention is missing, explicitly label the first observation of the new diagnostic/schema as a rollout proxy.
- After sufficient production volume, compare first-call error rate and immediate retry success with numerators, denominators, model controls, confidence intervals, and sensitivity windows that exclude development dogfooding.

## Acceptance Criteria

- [ ] A reproducible baseline documents patch call count, marked-error count/rate, missing/non-unique breakdown, retry success, model attribution quality, infrastructure exclusions, classifier coverage, and observation window.
- [ ] A bounded trajectory analysis identifies the dominant recoverable causes rather than inferring them solely from aggregate error strings.
- [ ] The selected implementation directly addresses one or more dominant failure causes and explains why it is safer and higher leverage than adding unrestricted fuzzy matching.
- [ ] No single-site operation silently chooses among multiple candidates, and only explicit `replaceAll` may modify repeated matches.
- [ ] Missing/non-unique diagnostics are bounded and provide actionable recovery information that can be consumed without an additional broad search/read cycle for the targeted cases.
- [ ] Error representation does not create parallel authoritative forms of the same recovery data; typed data and rendered text have clear, non-overlapping consumers.
- [ ] Existing patch atomicity, simultaneous original-content resolution, overlap rejection, clipboard rollback, and worktree scoping remain intact.
- [ ] Focused unit/property/trajectory tests cover the selected behavior and output bounds.
- [ ] Normative patch specs and executive status accurately reflect the implemented behavior and pass Allium/spec validation.
- [ ] A post-deployment measurement procedure is recorded with exact-SHA preference, proxy labeling, model controls, uncertainty, and dogfooding sensitivity windows.
- [ ] `./dev.py check` passes.

## Success Metrics

Evaluate after enough post-deployment volume for a meaningful comparison:

- primary: adjusted patch `is_error=true` rate per patch call;
- recovery: successful next patch call within ten minutes after a missing/non-unique failure;
- guardrail: no increase in wrong-site edits, overlap violations, atomicity failures, or unbounded diagnostic output;
- reporting: absolute and relative changes with denominators and uncertainty, both overall and for stable model cohorts.

The task is successful when it ships a safe, evidence-backed improvement and a credible measurement plan. A statistically conclusive production reduction may require observation after the implementation task is complete and must not be fabricated from insufficient volume.

## Out of Scope

- The `phoenix-tool-review` skill, tracked separately in task 56005.
- A general production analytics dashboard.
- Automatically applying a best-guess ambiguous match.
- Replacing the patch tool with AST-specific editors.
- Optimizing unrelated path, browser, bash, or sub-agent failures.
