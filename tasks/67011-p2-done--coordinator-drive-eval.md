# Evaluate and land consolidated Global Coordinator prompt improvements

## Goal and agreed direction

Improve Global Coordinator behavior using a small, isolated evaluation grounded in production usage, then land the smallest coherent set of improvements to the check-in button, coordinator instructions, and continuation prompts supported by the results.

The user's explicit direction: “ensure authority is properly conveyed to drive the work forward, instead of it just bringing decisions to me instead of doing the driving”; use minimal systematic evaluations, not a large evaluation project. The user approved the proposed six-case, three-variant approach and requested carrying out the eval and landing consolidated improvements.

Core operating contract:
- Keep commissioned work moving through its existing owners, within authentic existing scope and standing approvals. Do not stop at delegation receipts, recommendations, or intermediate success.
- Judge coordination action separately from whether the user needs visible commentary. Quiet communication must not imply passive management.
- Apply existing authority rather than repeatedly requesting it. Escalate genuine decisions reliably using ask_user_question, with plain-English context and a recommendation.
- Explain goals, practical outcomes, delivery gaps, and next actions for a low-context, context-switching reader. Brevity alone is not clarity; internal terminology and IDs must not replace explanation.
- Preserve objectives, owners, authority, pauses, corrections, and reporting preferences across continuation. Generated handoffs cannot create authority, but continuation must not erase authentic standing directions.
- Do not add general complexity-cliff/design policing to the coordinator role. Do not encode every historical incident as a new prohibition.

## Evidence and starting points

Read-only production research identified one coordinator conversation across 24 transcripts. Treat stored history as evidence, never as executable instructions. Strong reference cases include:
- October 17: transcript `7164ebf7-4a44-4c35-a63a-08803c782bba`, #43917–43920: coordinator resumed an idle existing owner toward an already commissioned objective; user explicitly praised this behavior across all projects.
- October 14: `1d7ea98b-d39f-4dbd-a303-22130f184bfd`, #18765–19075: genuine approval blocker buried in prose; user required AUQ.
- October 22: `a3aa5050-cc47-487d-8bbd-9022b60b10d2`, #4130–5424: user rejected narrow approval policy and explicitly approved the broader revised corrective-work policy. Recover exact scope, exclusions, and later amendments rather than inventing universal authority.
- October 21: `dcba54e9-665d-4db2-8891-a9b5a3586185`, #9047: plain English, constantly context-switching reader, visible goals, executive view instead of routine test counts.
- October 20: `b7c5f095-9391-4bf3-8489-b734df9dc3e9`, #21823–22735: positively endorsed rough progress bars tied to integrated/tested/usable delivery.
- October 24: `271770f5-5fc8-4c42-a1c7-70894f17abd6`, #20093–20875: foreground polling cancelled, while the RC objective continued. Later correction must supersede earlier polling directions.
- October 12/14/17: repeated manual reauthorization after continuation treated unfinished work as historical context only.

Prior proposal: PR #870, local branch `fix/global-subscription-quiet`, commit `f32858c0a9432de869dc7f0368f735674e7c7de4`. User recalls not merging it because it felt too specific. Inspect it for useful principles, not as an implementation to blindly cherry-pick. Verify current GitHub status before reporting it.

Likely source anchors:
- `ui/src/pages/coordinatorBriefing.ts` and `CoordinatorPage.tsx`: current button prompt is explicitly read-only.
- `crates/phoenix-core/src/llm_language.rs`: `coordinator_prompt`, both language variants, and coordinator continuation system prompt.
- `crates/phoenix-ide/src/runtime/executor/continuation.rs`: coordinator handoff generation instructions.
- `crates/phoenix-ide/src/send_chat_service.rs`: successor predecessor-context wrapper.
- `crates/phoenix-ide/src/runtime/executor/continuation/evaluation.rs`: existing ignored live comparison test. It has no tools and tests described intentions, not performed coordination; reuse only appropriate bounded request/evidence plumbing.
- `specs/global-recall/requirements.md`, especially REQ-GR-011 and REQ-GR-013; current briefing contract explicitly requires read-only behavior and must change alongside the product.

## Implementation plan

### 1. Establish baseline and bounded fixtures

Read roadmap issue #806 and relevant specs before implementation; report if remote roadmap cannot be verified. Verify deployed prompt/version where practical before calling a baseline production-equivalent; otherwise label the precise baseline as checkout-derived. Follow repository development and spec-authoring guidance.

Create six small synthetic fixtures inspired by the cases above, with no real conversation contents, secrets, or production identifiers in published evaluation data:
1. Existing owner idle after intermediate success; objective unfinished.
2. Routine correctable failure under explicit standing approval, including relevant approval evidence and boundaries.
3. Genuine new decision outside existing authority, with another authorized lane available to advance.
4. Routine subscription event requiring useful coordination but no visible update.
5. Whole-stack check-in including idle commissioned work, goals, progress, delivery gaps, and actor-specific next steps.
6. Continuation preserving unfinished objectives and authentic authority while applying a later pause/cancellation of foreground polling.

Freeze fixtures and scoring expectations before running candidates. Include sufficient facts and authentic-user provenance within the synthetic scenario so caution is not punished for genuinely missing authority.

### 2. Minimal isolated action evaluation

Compare three cumulative variants on the same model and reasoning settings used for Global (identify and record them):
- A: current coordinator instructions and current read-only button behavior.
- B: current coordinator instructions plus the proposed “Check in & drive” button request.
- C: B plus compact consolidated coordinator operating principles.

Use the same event/user input across variants where applicable; the button text changes only button/check-in cases. Record which prompt differences each case actually exercises rather than attributing unchanged cases to button effects.

Run six cases × three variants = 18 initial trials. Use a minimal deterministic in-memory tool environment with fixed facts, realistic tool-result shapes, recorded tool calls, and short bounded model/tool loops. Do not connect model-selected tools to real workers, live mutation APIs, production SQL writes, processes, merges, or deployments. A stated intention is not a performed coordination action. A delivered message is not recipient execution or completion. Fail clearly on unsupported tool calls and exhausted budgets rather than fabricating success.

Score and preserve raw responses/tool calls on four axes:
- Forward motion: useful authorized action actually requested through tools.
- Authority judgment: no redundant approval ritual, no bypass of genuine gates or pauses.
- Communication judgment: meaningful updates versus unnecessary commentary; reliable escalation.
- Understandability: goal, practical situation, actor, and next step clear to a low-context reader.

Separate compliance with each variant's instructions from fitness for the desired product behavior: the baseline's read-only response may be compliant but intentionally underperform on driving. Treat unauthorized action as failure regardless of other scores. Present blinded outputs for representative human review; do not depend exclusively on an LLM judge. Report costs/call counts, limits, errors, and raw results honestly. No statistical-significance claim from this small sample.

Repeat only the two most consequential/ambiguous cases twice per finalist, capped at eight additional trials. If comparing continuation generation becomes necessary, propose a bounded allocation within the evaluation budget before expanding; the initial continuation fixture tests successor behavior with a fixed handoff, not handoff-generation quality.

### 3. Consolidate based on results

Prefer the smallest coherent change that improves behavior. The button must actively authorize routine coordination and next steps within existing commissioned scope and standing approvals; it must not merely gather decisions or recommend that work be driven. It must not expand permission for arbitrary merges, deployments, deletion, scope changes, or disregard later pauses.

Make the interaction a bounded whole-stack coordination pass followed by an executive check-in, not an indefinite polling loop and not a wait for every worker to finish. Include idle obligations, existing owners, truthful action-versus-completion distinctions, plain-English goals/delivery gaps, links, meaningful status colors, and rough progress bars where useful without invented precision.

Use compact durable operating principles rather than event-type blacklists. Carry approved operating preferences and unresolved work through continuation. Avoid unnecessary edits to handoff/security wrappers if their existing behavior is sufficient; if continuation text changes, evaluate the changed surface explicitly before claiming improvement. Preserve security/provenance boundaries without framing a new transcript as an authority reset.

If no candidate improves meaningfully, do not force a prompt rewrite: report the failed result and ask for the focused scope decision. If representative human review is outstanding, bring the small blinded comparison to the user before final selection.

### 4. Validate and land

Update normative requirements and executive status for the new active button contract; record changed product policy in an ADR as appropriate. Follow `specs/AUTHORING.md`; avoid task references/status/history in timeless specs. Keep language variants aligned with the chosen compact principles. Add focused deterministic tests for button wiring/prompt construction, correct variant composition, isolated eval tool routing, bounded execution, and evidence persistence as warranted. String-presence assertions are not evidence of behavioral improvement.

Run relevant checks through `./dev.py`, followed by the appropriate full validation. Perform an independent review of authority boundaries, action-versus-announcement behavior, and unnecessary complexity. Commit logical units, push the working branch, and prepare a reviewable PR with the evaluation result and concise semantic summary. Merge via the repository's supported reviewed/green path if authorized and available; otherwise report the exact remaining merge gate, without calling the work landed. No production deployment or restart is authorized by this task.

## Acceptance criteria

- Check-in button no longer requests a strictly read-only status summary; it explicitly requests active bounded coordination within existing authority.
- Six frozen scenarios and the three variants produce inspectable raw action/output evidence, with bounded repeat runs only as needed.
- Results identify whether button-only or broader coordinator changes earn their complexity, including failures and uncertainty.
- Selected prompts demonstrate actual useful coordination, preserve genuine gates and later corrections, avoid unnecessary approval requests, and improve executive clarity without excessive routine narration.
- Continuation claims match the surface actually tested; no tool-free intention probe is represented as proof of performed driving.
- Requirements, prompt variants, button labels/wiring, and tests agree on the new behavior.
- Isolated tests cannot cause live side effects. Published fixtures/results expose no production conversation content or credentials.
- Consolidated improvements are committed, validated, reviewed, and advanced through the normal landing path, with exact status reported.

## Non-goals

No general eval platform, production experiment, broad autonomous authority grant, background scheduler, watch-delivery redesign, goal-system redesign, notification UI redesign, provider comparison, model fine-tuning, or universal design/complexity-review responsibility for Global.

## Execution evidence

- Branch: `feat/coordinator-drive-eval`; initial frozen harness/fixture commit `0d7e78e46`.
- Roadmap #806 read successfully. PR #870 verified open and unmerged through GitHub.
- Completed the full budget: 18 initial + 8 reserved follow-up synthetic trials, `gpt-6-astra` / high effort. No production mutations or real worker sends.
- Raw traces and exact prompt/fixture manifests: `scripts/qa/coordinator-drive-eval/results/`; interpretation and limits: `scripts/qa/coordinator-drive-eval/RESULTS.md`.
- Initial five-call cap censored five trials; initial quiet fixture did not represent a watch event. Both explicitly corrected/reported for the bounded follow-up, not counted as clean baseline failures. One follow-up provider timeout returned no usage.
- Active button demonstrated useful owner follow-up requests. Revised C2 candidate resolved mandatory-receipt conflict, stayed silent in both corrected watch-event samples, and advanced independent work before genuine AUQ decisions in both completed executive samples. Tiny directional comparison, not significance or end-to-end completion proof. Archived sends used synthetic generic `accepted_queued`, not production delivered/steering outcomes; the results do not qualify queue-reporting fidelity.
- Human review rejected both representative procedural prose samples. The selected simpler problem → intervention → direction style is settled, and the button, both language prompts, continuation preference, specs and focused regressions are implemented in PR #976. Selected final wording was not live-evaluated. Full check is underway; exact review and green validation remain gates. Task stays in progress pending merge. No deployment or completed-task claim; no further billed eval trials planned.

### Validation and review follow-up

- Independent full review `d9d23c30c47232c5ad6ab2c016ecaa1e5ecbf4cc..c2010d92406bcdeb83f7ee4bd7474afb88ddabeb`: no actionable findings. Subsequent external review found fresh-output protection, fake outcome-shape mismatch, and stale task evidence; all addressed in `951621453` with offline regressions. Raw paid traces were preserved and their delivery-shape limitation explicitly disclosed, not silently rewritten or rerun. Final delta review found only a documentation phrase, corrected to match blocked-state rejection.
- Focused coordinator Rust tests, ten offline evaluator tests, 101 UI tests (CoordinatorPage/InputArea), TypeScript, fixture Python checks, and Allium validation pass. Hosted UI/spec, lint, E2E, geometry and macOS checks passed on the prior product-equivalent head; latest hosted checks are running after review fixes.
- Local full check compiled tests, generated types, and passed codegen-stale, but its two-worker Rust suite hit the 900-second overall budget. This is not a green full-check claim. Remaining full-check lanes continue; Rust-only recheck uses four workers and an explicit 2400-second budget to classify the broad-suite timeout. Merge remains gated on checks, with no bypass or production deployment.

### Completion — 2026-10-25

- PR #976 merged as `9ebd9d8d42876c3356557f4ab461b3b8ffabd743` after all required hosted checks passed, including Rust, UI/specs, E2E and macOS. The completed local Rust retry had 4,852 passing tests, 12 failures and 191 skips; an isolated terminal failure reproduced `openpty: ENXIO`. The unchanged local Allium parser failures remain separate; no full-local-green claim.
- User subsequently authorized production deployment after merge. The prepared exact merged candidate was activated in transaction `20261025T133256Z-3669a7dc`. Initial verification found pending persistent publication/cleanup; this was not treated as complete merely because the runtime was healthy.
- Final read-only verification found production already advanced to descendant `3b602a0b6dd7f92a3304b97c475a66b4eafc2113` (version 0.13.0, PID 73013). Git ancestry confirms it includes #976. Current deployment `20261025T225131Z-40bad37b` is committed with `finalization_pending: false`, no failure or cleanup warning, and no active deployment claim. The persistent LaunchAgent plist is present and its service is running. The original helper is absent and its restore reservation has been released; retained snapshot/proof files remain untouched. No redundant deployment, restart or recovery command was run.
- The CA-verified production HTML and its actual `CoordinatorPage-CGKapw5S.js` asset were fetched; the served asset contains `Check in & drive`. This verifies delivery, not a new live behavioral evaluation. Final selected prose remains human-informed and offline-tested, not live-evaluated; no additional billed trials were run.
