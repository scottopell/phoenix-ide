# Implement WorkScope load indicators

User requested this completed design be added to the implementation queue on 2026-10-09. Queue as ready; do not start automatically or preempt active delivery. No merge/deployment approval is implied.

Design owner and source: @conv:2dd783fa-c469-4510-9ac0-a29af03cd2e5
Final baseline: @transcript:96ccb8a6-5bd1-4a36-a636-ffc879645790#message-0d256e9d-11ac-4daf-b19d-886ba9d295b5
User tool/API decision: @transcript:96ccb8a6-5bd1-4a36-a636-ffc879645790#message-96ccb8a6-5bd1-4a36-a636-ffc879645790:dc7aa0bd-0a71-4d2d-8121-4cfe639723c4

## Agreed scope
- First-party API for one WorkScope and a breakdown across WorkScopes, plus a Global-Coordinator-only tool. Both consume the same underlying resource summary.
- WorkScope-wide accounting regardless of initiating conversation: Global-launched jobs count under their target scope. Deduplicate attributed processes; no second coordinator total.
- Existing owned bash process groups define attribution. Detached descendants are excluded; make measurement boundaries explicit.
- Always-on CPU observations while tracked jobs are live, targeting roughly five-second intervals with resource-aware collection.
- CPU as cores consumed: latest interval and 1/5/15-minute averages with actual available history coverage. History is in memory only; server restart resets coverage.
- Fresh shared demand-driven memory samples, retaining platform semantics and explicit unavailable values.
- Small inline UI indicators, not a dedicated page or modal.
- Reuse existing collection infrastructure, not a new monitoring framework.

## Explicit exclusions
Disk measurement/caching, run-queue metrics, host diagnostics, reclamation, automatic resource controls, persistence/recovery for CPU history, and automatic conversation-context injection.

## Implementation and acceptance
Resolve exact API/tool shapes, API access rules and inline placement within this baseline. Preserve ownership versus control distinctions. Test shared-scope/coordinator attribution and deduplication, CPU windows with partial coverage and restart reset, demand-driven memory/unavailable semantics, and Global-only tool exposure. Verify inline indicators in the actual UI. Do not broaden to host monitoring or disk cleanup because those motivated the original discussion.

Retain the design conversation as the implementation handoff; coordinate scheduling through the existing project delivery queue and roadmap #806.

## Implementation progress — 2026-10-11

Explicit implementation handoff and attached task approval supersede the earlier queued/not-launched stopping point. Branch `feat/workscope-load-indicators` was created from verified remote main `c5c33a53`; the attached task commit was preserved. No merge/release/deployment authority was exercised.

Implemented the shared runtime-owned service, individual/across-active-scope API, execution-admitted Global-only tool, generated summary types, and existing-section inline indicator. Added requirements and ADR-092 to explicitly change the demand-only CPU contract while keeping memory/host demand-driven. Independent reviews identified and corrected incomplete totals, process-incarnation cache keys, conflicting ownership, separate CPU freshness, unknown scope success, and idle zero presentation.

Validation:
- Focused Rust `work_scope_load`: 16 passed (includes HTTP/tool admission).
- Shared `resource_monitor`: 4 passed.
- Focused UI WorkScopePanel/FileExplorerPanel/ConversationPage archived/live integration: 120 passed, 2 pre-existing skips. Fixture warnings remain; no test failure.
- `./dev.py check --lanes tsc,ui-lint`: all 3 checks passed.
- Codegen passed; Rust formatting, diff whitespace, and 1090-task validation passed.
- Broader `./dev.py check --lanes rust`: 2285 passed before failing `bash::registry::tests::cascade_bash_on_delete_records_live_pgids_and_drops_entry`; 2363 tests not run due to fail-fast. Isolated rerun reproduces expected2/actual1 failure. Owning registry source is byte-for-byte unchanged from starting main (SHA1 88a561fedede2b519cc91f1c4dff4b6aa884d4c6); classify as unrelated blocking qualification, not a reason to silently alter lifecycle tests here.

Remaining qualification: exact-head hosted CI/review, broad Rust baseline failure disposition, and rendered browser acceptance (unit DOM tests are not visual acceptance). Keep task in-progress until those gates are satisfied. No production profiling, process termination, cleanup, host configuration, or deployment occurred.

Coordination: attempted handoff status to the supplied Global conversation reference was rejected as target_not_found; no delivery claim. Concrete UI edits are bounded to WorkScopePanel, its FileExplorer/DesktopLayout wiring, and tests; no transcript rendering/links/details or replay/startup reducer rewrite.

Published draft PR [#888](https://github.com/scottopell/phoenix-ide/pull/888) in separate backend/spec and inline-UI commits. Final all-target `./dev.py check --lanes clippy` passed after correcting new fixture assertions/checked-time arithmetic and a boolean simplification. Hosted CI remains authoritative for exact published-head qualification. Roadmap worker status posted at comment 6114516361 (projection acknowledgement pending at posting).

## Continued qualification — 2026-10-11

Earlier unchanged-source classification was insufficient and is superseded by equivalent exact-base comparison. Archived `c5c33a53` under ignored `.phoenix/qa-base-c5c33a53`, using Rust1.95.0/shared absolute CARGO_TARGET_DIR, --locked and exact single-test --test-threads=1, then candidate0150d96c: both fail registry.rs2066 with kill_failures expected2/actual1 (0passed/1failed/535filtered). Logs `.phoenix/qa-base-c5c33a53-test.log` and `.phoenix/qa-candidate-0150d96c-test.log`. Cause: macOS Unsupported retirement records first failure then breaks; subsequent absence adds none. Toolchain/config/lock/owning crate and registry match. phoenix-tools does not depend on phoenix-ide; the runtime-owned sampler is not constructed on this isolated test path. This is a separately reproducible baseline macOS qualification obstacle, not a candidate regression.

Hosted0150d96c: Rust, Clippy, E2E and build passed; ui/spec lane failed on this PR's ADR missing exact Options considered heading. Corrected heading and reran spec-shape/anchors. Independent fresh review found missing launch leader proof: numeric PGID members were accepted if old launch leader absent. Both CPU/memory now fail closed unless launch PID/start match, with deterministic surviving-member tests; no expanded descendant tracker. Parent authenticated GraphQL fetched all reviewThreads(first100), hasNextPage=false, zero threads at0150d96c (nested pages absent). Public REST review/issue feedback empty; exact-head refresh required after qualification commit.

Actual browser QA used local dev.py servers only (8019/8035) and real `/c/review-the-recent-changes-to-the-fde0` owning path. Real API idle200/zero confirmed before deterministic intercepted20-state matrix: active/idle/stale/unavailable/partial × dark/light ×1440/1100. No JS errors/overflow; keyboard Enter Work collapse/reopen; real canonical pinned historical route made0 load queries over6s. Evidence `.phoenix/qa-load/report.json`, script and22 screenshots (harness-local status pointers, not public evidence). Parent visually inspected active dark, active light and partial dark screenshots. No archived DB row available; archived path integration tests remain, not rendered archived-row acceptance. Browser found/fixed poll-start versus response receipt timestamp freshness race and low contrast in newly owned load values; delayed-response test added, secondary-text styling used. Final focused UI121 pass/2existing skips. CPU/memory leader-proof focused Rust12+monitor5 pass, all-target Clippy passed; tsc/lints/spec checks rerun.

Startup integration boundary: service is runtime-owned and timer installed in RuntimeManager constructor, weakly held, with native sampling skipped when no live groups. Current main has no observation-only execution capability constructor; standalone API startup observation helper does not construct RuntimeManager. Perf's future execution/observation split must construct this service only in execution capability branch, not observation startup. Attempted coordinator relay to supplied exact transcript rejected coordinator_chain_rejected; surface this seam in final handoff rather than invent parallel capability types or broad startup refactor.

## Ready-for-review hosted feedback

Scoped user approval via Global authorizes ready→review-clean→land→deploy; Global is designated sole merger, release/host operator routing remains Global-owned. Promoted ready and requested hosted review; completed review5485544972 at6c56d455 found3 concreteP2s, all accepted: remote-browser clock skew, transient jobs invisible to endpoint launch-set equality, and CPU timestamp emitted for unavailable latest. No merging while findings outstanding.

Fixes: UI monotonic receipt expiry and server-domain CPU timestamp delta; scope lifecycle epochs structurally bound to registry handles (insert/remove/terminal edges), bracketed during native snapshots and checked at summary to invalidate crossed intervals; null latest means null CPU timestamp. Regression coverage ±1hr clock skew, delayed receipt/expiry, transient job crossed interval and subsequent stable recovery, failed timestamp. Focused Rust19 pass, all-target Clippy pass, registry37/38 with retained known baseline macOS assertion, focused UI89 pass. Read-only lifecycle epoch review found no concrete blocker. New exact head requires hosted/independent rereview, CI and full current thread dispositions before landing.

## User-selected sampled contract — 2026-10-12

User explicitly selected SHIP SAMPLED VISIBILITY: lightweight indicators for live tracked jobs, not exhaustive accounting; short-lived native children may be missed; attribution ends with tracked handle; sampling duration is not completeness. Preserve concrete lifecycle/identity/memory/freshness fixes. API replaces misleading coverage_seconds with sampled_seconds and exposes typed measurement_basis live_tracked_bash_groups. Tool/UI explain boundaries and no-tracked-live-jobs zeros. Requirements aligned and new ADR096 records decision; resource ADR092 moved to095 to avoid authorized incoming federation092–094, subject to exact actual-main reconciliation.

Coordinator landing sequence890→891→860→qualified888; Global solemerger and solehost route, no performance branch merge. Current sourcebasec5c33 must be reconciled with actual mergedmain and combined active/pinned/archived/close-retry/reentry polling/admission tests before final clean receipt. Sampled successor focusedRust21pass, UI113pass/2skip, codegen/type/lint pass; hosted re-review and full thread dispositions pending publication. Scope decision is not a waiver for detected race bugs.

ADR allocation correction: current base's validator requires a gap-free chain, so resource092 and sampled093 remain sequential until incoming federation092–094 actually lands. Then rename both resource decisions to095/096 as part of cumulative main integration; do not publish speculative gaps. Sampled successor Clippy/spec-anchors passed; gap-free spec-shape passed.
