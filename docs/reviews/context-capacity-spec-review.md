# Context-capacity specification adversarial review

Reviewed 2026-10-25. Independent reviewer received only the implementation plan, proposed specs and neighboring normative contracts; no runtime code, production logs, or author conversation. This is a specification review, not proof of runtime correctness. No production changes were made.

## Six highest-leverage findings

### 1. A request boundary is not necessarily a safe model-switch boundary

REQ-LLM-004l originally promised switching at the next request. Existing exchange ownership can still require signed/private replay at the request returning tool results. Responses-to-Anthropic switching could drop required state prematurely. Draft repaired to require exchange settlement or authoritative abandonment. Remaining acceptance obligation: specify pending tool-result handling and user-visible deferred switch behavior; no rerunning tools or silent model substitution. This is ordinary integration work unless it expands into a new switch workflow.

### 2. A valid handoff may strand its successor

REQ-LLM-004f/004j and REQ-BED-020a reserve full output for both requests, but do not relate generated summary size to successor input capacity. Example: context 128K, output reserve 64K, ordinary fixed overhead 10K, handoff 60K. Summary generation may succeed but the ordinary successor cannot fit its protected opening. A successful summary is not a progress guarantee.

Unresolved implementation gate: choose either a successor-aware handoff sizing contract or a visible pre-admission capacity conflict. Do not add a silent fixed summary cap, recursively compact, or clip accepted context. Recommended narrow direction is pre-admission fit validation with actionable conflict, followed by a deliberate size policy only if evidence requires it. This needs an explicit normative follow-up before implementation.

### 3. Lossy selection cannot guarantee preservation of all obligations

REQ-BED-020a originally promised semantic preservation while REQ-BED-020 explicitly retains only protected opening plus bounded recent suffix. A user correction between them can disappear before summarization. Adding an obligations database to fulfill the promise would violate agreed scope.

Draft repaired to protect the accepted opening and require summary instructions over selected evidence, explicitly disclaiming lossless memory. REQ-LLM-004l preserves portable history across a switch, not an unlimited active prompt. Semantic fixtures must still test corrections, uncertainty, provenance and retrievability; evaluation failures may motivate a separately reviewed selection change.

### 4. Frozen compaction retry can be permanently impossible

REQ-BED-020 freezes retry input. New capacity policy forbids retrying an unchanged impossible plan, but “revised-input/model recovery” does not define how a frozen operation accepts a revision. Oversized protected context could expose a useless Retry action forever.

Unresolved implementation gate: explicitly distinguish transient generation retry from deterministic fit conflict; define authorized model/input revision, operation identity and generation fencing for old results. Reuse existing durable continuation recovery where possible; do not assume it already supports this revision. Recommended no-progress behavior is no unchanged retry, with a clearly identified supported intervention. This needs a normative transition before implementation.

### 5. Per-candidate timeout conceals an unbounded counting loop

One two-second call per candidate permits 100 candidates to cost 200 seconds. This is a complexity cliff if implemented as a scheduler or durable counting job.

Draft repaired to at most one remote count and one two-second timeout across the logical planning operation, including clearing and handoff planning. Subsequent candidates use local fallback. Provider latency suitability remains unmeasured; two seconds is an explicit engineering default, not a provider guarantee.

### 6. Provider Allium contradicted the output policy

Anthropic request translation still described clamping output by remaining context while REQ-LLM-004f prohibited silent shrinkage. Following either normative artifact produced different behavior.

Draft repaired: translation consumes the capacity-approved declared reservation. Other bounded utility contracts remain valid. This is cross-spec drift, not a reason to add another budget abstraction.

## Delivery disposition

Plan and spec draft are saved for review. Findings 2 and 4 are implementation blockers; finding 1 requires explicit boundary acceptance scenarios. Findings 3, 5 and 6 have drafting repairs. Do not label this package runtime-ready or deploy recovery from it. No Allium open-question declarations were introduced; unresolved design gates are tracked here and in the task, rather than treated as settled normative behavior.

## Validation

`./dev.py check --lanes spec-shape`, `./dev.py tasks validate`, and `git diff --check` pass. The modified Bedrock Allium file reports zero errors. Anthropic Allium reports two existing parser errors at the unchanged qualified contract references `fulfils llm/ToolHistoryIntegrity` and `demands llm/ProviderRequestProjection`; the base commit contains the identical constructs. No claim is made that the complete Allium corpus passes. Runtime tests were not run because this change is documentation only.

## User resolution: findings 2 and 4 recovery direction

2026-10-25 voice review approved same-model ordinary continuation and full output reservation, with successor working room beyond bare fit. Exceptional capacity conflict offers original-preserving manual editing, one user-requested AI shortening candidate reviewed before replacement, or explicit model change. No automatic rewrite loop; candidate and rewrite request must fit; no silent source clipping. REQ-BED-020b records this decision. The exact working-room target and revision/generation-fencing transitions remain implementation specification gates; 40% is not a normative threshold.

## Model-switch finding correction after implementation verification

User challenged the mid-tool scenario. Read-only verification of the deployed baseline found `ConvState::allows_model_change` admits only Idle, Error and RecoverableContinuationFailure, and the API checks effective state. REQ-CONV-010h already prohibits queuing locked configuration changes; REQ-LLM-004c did not enumerate the states. The draft's next-request wording created the ambiguity. Added the exact admission whitelist to REQ-LLM-004c and cross-referenced it from 004l. No deferred-switch feature is required or authorized. Original finding 1 is a spec omission/draft regression, not an established runtime defect.

## User resolution: finding 5 deadline

2026-10-25 voice review delegates the numeric counting timeout to implementation informed by real provider routes. The normative contract requires a finite absolute timeout, at most one remote attempt per logical planning operation, no retry and visible local fallback; two seconds is no longer a requirement.
