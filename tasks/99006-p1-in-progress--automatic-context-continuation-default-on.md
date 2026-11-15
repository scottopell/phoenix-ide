# Default automatic context continuation ON for new conversations

## Commission and observed journey

The user explicitly requests automatic context continuation ON by default so forgotten toggles do not cause context-exhaustion stalls. New ordinary ProductConversations and a newly initialized Global Coordinator must start enabled across supported web/API/native/bootstrap creation paths. Existing persisted preferences, especially explicit false, remain authoritative. This is context-exhaustion continuation, not permission for unrelated work, unconditional retries, Goal creation, or immediate dispatch.

## Verified starting evidence

Explore checkout is clean, detached at bc92612f1c2b39fdbffd0499188c7b5192513142, matching local origin/main. Verify remote current main before implementation and use an isolated owned branch. Roadmap #806 generated body was read via GitHub; it is coordination context, not a replacement for normative specs.

- `AutoContinueOnContextExhaustion` in phoenix-core domain product_conversation.rs and its regression explicitly default Disabled.
- `specs/bedrock/requirements.md` REQ-BED context exhaustion contract explicitly requires OFF; update that narrow product contract and record a new ADR superseding only ADR-054's default choice, preserving admission/authority rules.
- phoenix-db migrations.rs adds the preference as NOT NULL Boolean DEFAULT 0. Existing rows have no provenance distinguishing implicit OFF from explicit false: preserve every existing row, with no enabling backfill.
- New ordinary creation in phoenix-db lib.rs, staged product_creation.rs, and Coordinator initialization in lib.rs omit the column and rely on SQL default. Audit all production insertion paths, including generic insert_conversation_tx and workflow paths; distinguish fresh aggregate creation from recovery/import/successor behavior.
- Continuation creation reuses inherited ProductConversation identity. Preferences belong to the stable aggregate; no successor reset or opt-out loss is allowed.
- AutomaticContinuationControl.tsx reads the backend preference and already explains prospective-only behavior; retain truthful loading/error states and opt-out. Do not replace unknown persisted state with optimistic ON.
- Existing DB/native/API/web/federation regressions cover settings and prospective non-dispatch behavior and contain assumptions about initial OFF; adapt fixtures intentionally, not by blanket assertion substitution.

## Interaction map and implementation scope

Web/API/native creation producer -> typed creation request/service -> fresh stable aggregate insert -> persisted preference -> settings read/projection -> UI/native observation. Coordinator bootstrap -> create-if-absent aggregate initialization; existing singleton returns unchanged. Context exhaustion -> existing atomic admission -> shared successor operation -> same aggregate preference. Synthetic federation destination -> destination-owned settings read/set; no saved-peer mutation.

1. Verify current main and establish an isolated owned branch. Read owning specs and development guidance. Check concrete overlap with mobile continuation/SSE owner @conv:f24470b4-f85f-48c7-b7bb-0bdaa58a8d49 (#1080); do not take over live-follow. Native approval/unified Close owner @conv:5d57fcc6-e336-469f-850b-63cc253ea0ea (#1058) retains separate source. Coordinate only concrete shared-file conflicts through available authorized mechanisms; do not message Coordinator chains.
2. Make fresh eligible aggregates initialize Enabled using a narrow creation-bound policy. Keep omitted input distinct from explicit false wherever a supported creation contract accepts this preference. If creation has no preference input, preserve its contract unless a small justified addition is necessary, and test explicit false through the existing settings path. Do not add broad rollout/recovery machinery or rewrite historical migrations to change existing rows.
3. Preserve existing persisted true/false settings across load/restart and Coordinator repeated initialization. Do not infer intent from false, wake/rearm exhausted work, or mutate existing peers.
4. Keep subordinate/sub-agent ownership restrictions, Stop, pause, approval, history, Close, generated-context authority, retry bounds, and no implicit Goal/dispatch semantics intact.
5. Update normative default language, add a focused ADR, and update executive evidence honestly. Clarify UI semantics only if needed; no redesign or live-follow fix.

## Required regressions and qualification

- Fresh ordinary aggregate default true through each distinct supported creation route; compare omitted input and explicit false where applicable.
- New-instance Coordinator initializes true; repeated bootstrap preserves explicit false and existing true.
- File-backed reload/restart retains false and true; existing-record upgrade preserves all existing preferences, including indistinguishable historical default false.
- Continuation successors retain the same stable preference, especially opt-out, without resetting it to a new default.
- Web control accurately displays returned ON, allows OFF, and retains truthful unknown/error presentation. API/native reads agree with stored values.
- Synthetic remote destination settings reflect destination defaults and preserve explicit false without enrolling or changing real peers.
- Existing gate/no-dispatch regressions remain meaningful; OFF-dependent tests explicitly establish OFF instead of accidentally depending on former defaults. No Goal creation, dispatch, or old-work revival merely from initialization/settings changes.
- Independent adversarial review of exact final head, with findings fixed and relevant tests rerun.
- Use existing hosted CI mechanisms for Rust/native/full qualification; wait with explicit structured handles/results where available. Record exact source SHA and actual job conclusions, not enqueue receipts. Verify compatibility with current main and repeat exact-head qualification after any source-changing restack.
- Deliver integration packet: PR/branch, exact final head, base/current-main relation, test commands/results and hosted run links, review findings/disposition, resource/platform limits, and no deployment claim.

## Safety and execution limits

Existing hold on local Cargo/rustc/native/full builds remains. Last commissioned disk report was 14.48 GiB free, not a current measurement. No new local dependency/browser environments without resource admission. Prefer source-only inspection, small existing checks, and existing hosted qualification. No production lifecycle fixtures, direct DB writes, config/trust changes, new dependencies/spending, release/deploy, shared restarts, or destruction of source/recovery assets. Report blocked capabilities rather than bypassing restrictions.

Do not invoke enable_auto_continuation: Coordinator reports this conversation's persisted preference already enabled. Historical call call_d1aa592c59d44d97acbf6fcc3e9991f4 wedged without output from 18:59:39 through 21:05Z and was cancelled solely for recovery; this is not proof of root cause and does not expand scope into lifecycle rewrites. Report a new execution wedge instead of duplicating its call.

## Unknowns to resolve in implementation

Remote-main freshness, precise native/API creation contracts, complete insertion-path inventory, shared-file conflicts, and hosted execution/tool admission remain to verify. Product policy itself is explicit: preserve old settings and apply ON only to fresh eligible aggregates. Escalate only a genuine incompatible product decision, not routine in-scope fixture/command repairs.

## Approval boundary

Explore mode permits this task draft only; implementation, commits, hosted dispatch, review qualification, and integration packet proceed after task approval grants Work mode. No implementation or tests are claimed by this proposal.
