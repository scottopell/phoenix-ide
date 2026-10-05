# Repair stale continuation opening settlement

## Commission and admission

One bounded successor SOURCE-REPAIR mission under Global's commission; not native or restart feature ownership. The project coordinator supplied API/scope inventory admission (not direct human DB verification): unique scope `a7988772`, managed Explore, owned worktree `e87be297-daa2-43f9-82e4-f72b7fe2421c`, `gpt-6.1-sol/high/Standard`, and accepted roadmap #806 owner record `5993991748`.

Checkout verified clean on `task-pending-e87be297` at `d6f94afaca8650fff75a638fd63c3e2c9eafc602`; cached `origin/main` matches. Explore network sandbox denied GitHub roadmap and remote readback. After approval, fetch/read back safe remote refs and verify the commissioned base without moving any checked-out main branch. Remain in the managed owned worktree.

`taskmd new --slug stale-continuation-opening-settlement --priority p1` was attempted and denied by the Explore filesystem sandbox. This plain approval brief deliberately claims no numeric ID: another retained worktree already owns task 93006. Immediately after Work approval, allocate the actual task atomically with `taskmd new`, retain all existing IDs, and place this plan in that allocated task. Do not audit or modify the allocator.

## Observed journey

The user reports supported chat returns HTTP 409 `continuation_opening_pending` for two retained owners:

- Native ProductConversation `00404b24-0927-43c1-952e-1be92e1f8c96`, latest/writable scope `6131f8f3-95d4-4b65-87c7-445574cef3c9`: idle, working false, 399 messages Oct 3–4; no outgoing successor, continuation operation, or automatic admission reported.
- Restart ProductConversation `df61fb90-126d-45a1-b6a5-8289ebd21bc5`, latest/writable `4e42c7b1-f249-414d-9c07-fd2391462cad`; retained source scope `06d4d773-e812-4036-8f3e-fb3bd37046ef` also returns that 409.

These are reported production observations, not independently inspected database facts. Deployed source reference is `39b6c59db18686bfa5396905d78c3d5a3f962f09`.

## Verified findings and failure model

- `SendChatApplicationService::send_with_admission_guard` rejects a non-reserved opening request when `Database::has_pending_continuation_opening` returns true.
- That query checks an incoming `continuation_dispatch_intents` row for the successor without a `completed_continuation_handoffs` receipt for its predecessor. Idle state and absence of an outgoing operation do not disprove that incoming obligation.
- Migration 100's `consume_continuation_dispatch_intent` message-insert trigger matches opening identity, successor, message kind, payload, and authority, inserts a handoff receipt, and consumes the intent. The receipt validates predecessor topology and referenced messages.
- `insert_canonical_message_tx` writes the real prepared content. `canonical_message_id_for_turn` uses `successor:client-key`, a form the trigger explicitly recognizes. Do not assume an unhandled canonical-prefix mismatch.
- `generated_context_authority_survives_dispatch_settlement` and `continuation_creation_persists_dispatch_intent_atomically` cover current-serializer settlement but do not alone establish legacy upgrade coverage.
- `reconcile_legacy_half_committed_continuation_tx` decodes stored continuation content; inspect supported historical serialization and migration paths alongside durable opening acceptance.
- Exact relevant source files (DB migrations, DB lib, direct-turn persistence, send-chat service, continuation service, core message schema) are unchanged between deployed 39b6c59 and commissioned d6f94afa.

Hypothesis: an already accepted/persisted opening can remain classified pending across a supported legacy serialization/migration or interrupted settlement boundary. This is NOT proven for production or in a failing regression. Establish the precise cause before choosing a repair. Do not infer acceptance from idle state, transcript prose, or message count.

## Interaction map

Selected handoff and opening authority → atomic successor/intent reservation → send-chat reserved identity validation → durable direct-turn acceptance → canonical message materialization → atomic handoff receipt and intent consumption → ordinary chat admission.

Inspect interruption/recovery edges, durable replay fast paths, and runtime startup's pending-opening steering fence. Recovery must preserve the original accepted identity and payload rather than resend an opening or invent new authority.

## Bounded implementation and regression plan

1. Verify Work admission/base and atomically allocate the task. Read `specs/bedrock/requirements.md` (REQ-BED-021 and recovery obligations), applicable Bedrock Allium, `specs/compatibility/requirements.md`, and related migration/recovery tests before editing.
2. Obtain only necessary read-only evidence for the exact named scopes when supported: prefer bounded warnings/traces; if exact DB inspection is needed, verify the actual deployment/database target first and restrict output to identities, kinds, booleans, lengths, migration versions, and equality results. No secrets or full transcripts; do not guess a target or send coordinator-chain messages.
3. Add a regression representing a supported historical persisted continuation/opening and its real migration/materialization path. Compare reserved client key, canonical ID, content kind/encoding, authority, predecessor link, pending intent, and completed receipt. Demonstrate the stale gate or falsify this hypothesis. Where feasible, run the same reproducer against both deployed and commissioned source using isolated test data only.
4. Fix the smallest proven persistence/settlement/migration boundary. Preserve atomicity and exact accepted identity. If persisted supported rows need repair, provide a narrowly justified forward migration or existing supported recovery-path repair, not a manual production write or a new general compatibility subsystem. Update normative contract/ADR only if a guarantee must change; do not rewrite historical migrations as a substitute for upgrading existing databases.
5. Tests must show matching accepted openings settle exactly once, intent is consumed, receipt references exact accepted message and authority, and a subsequent ordinary chat on the same writable scope passes this gate. Verify interruption/reopen recovery without duplicate opening, transcript mutation, or fabricated identity. Exercise manual user-authorized and generated-context openings where relevant.
6. Negative tests: genuinely pending opening still blocks ordinary chat; wrong identity, scope, payload, content kind, or authority cannot settle or bypass the gate; close/history/work-scope authority fences remain intact. Include a service-level gate regression as well as DB/migration coverage.
7. Run focused tests and `./dev.py check`, review exact HEAD adversarially for authority and migration correctness, then normal branch commit/PR/CI review. Report source/regression evidence separately from production remediation. Global owns final publication, merge, and deployment according to actual runtime instructions.

## Acceptance and stop conditions

- A deterministic failing regression or equally precise evidence identifies the cause before the fix; passing regression plus negative fences validates the minimal repair.
- No claim that either retained production scope is repaired without authorized runtime evidence after Global's release/deployment.
- Preserve both ProductConversations, all history, original source worktrees, task IDs, settings, and model configuration. All writes stay in this mission's managed worktree and isolated test data.
- If the supported reproduction disproves the hypothesis or needs a broader continuation redesign/policy decision, checkpoint the evidence and request revised scope rather than bypassing gates.

## Explicit non-goals

No raw production DB lifecycle writes, opening replay, host restart/deploy, edits in either blocked owner's tree, native feature work, TestFlight upload, #788/design resumption, RC #844 work, broad continuation redesign, removal of authority gates, or config/model changes outside this mission's own admission. No duplicate owner or coordinator-chain messaging.

## Work checkpoint

- Authoritative read-only `work_scopes` query: a7988772-cccd-4bc5-9a30-6c33df4da552 is active/work/allocated_worktree; runtime developer instructions grant full writes in this managed tree. Branch is task-stale-continuation-opening-settlement-source-repair-e87be297.
- Approval committed the plain brief at 90f0a8f6a. Atomic allocator first returned 93006, colliding with a retained native-owner task outside this tree; a second `taskmd new` returned 93007. Only this mission's duplicate allocation and plain brief are removed, not any other owner's files. This is the mission's actual task ID.
- Safe fetch and independent `ls-remote` verified current remote main d6f94afa. Roadmap #806 accepted marker includes 5993991748; reviewed generated body.
- Read-only application DB query confirmed both writable successors have incoming manual intents and matching canonical user opening rows, exact text equality, valid predecessor linkage and a predecessor continuation row, but no completed handoff. The supplied third ID 06d4d773 was absent from conversations in this database. No transcript content or secrets queried.
- Opening timestamps: native 2026-10-03T04:19:32+00:00; restart 2026-10-03T20:53:56+00:00. Migration ledger: 045 on Aug 19, 074/100/110/112 on Oct 4 at 18:56 UTC. Thus both existing canonical openings preceded installation of the corrected trigger and receipt schema. Proven supported historical producer: #581 canonicalized messages while migration 045 matched raw keys only; #724 migration 074 fixes future inserts but lacks historical reconciliation; migration 100 preserves those intents as user-authorized.
- Added first focused forward-upgrade regression before any source repair. It installs migration 045, persists a canonical exact opening, observes stale intent, upgrades through 074/100 and the current runner, then asserts the same pending predicate must clear with exact receipt and no message replay. Initial compile/test is in flight.

### Regression and repair evidence

- Before repair: focused canonical historical upgrade regression exited 101, failing `already persisted exact opening must not fence subsequent chat` (1 failed, 0 passed).
- After migration 113: same focused regression passed (1/1); five historical migration tests passed; phoenix-db clippy all-targets passed. Real send-chat service module passed 23/23 including pre-migration 409 and post-migration Delivered, while missing/mismatched openings stay blocked. Further exact-candidate qualification is in progress.
- Migration 113 uses typed message deserialization, same-aggregate/root/role topology, manual authority, one exact raw/canonical opening identity (including conflicting kinds in ambiguity detection), literal non-meta user payload equality, one decodable predecessor continuation, and no competing receipt. Its receipt/intent/ledger commit is atomic. It changes no transcript row, durable turn, prompt, WorkScope, or settings. Generated-authority intents remain untouched.
- Negative and transaction tests cover identity/payload/kind/scope/topology mismatch, ambiguous identities/summaries, conflicting valid receipts, generated authority, byte-preserving idempotence, injected receipt/deletion abort rollback of multiple candidates, and retry. No opening replay or gate edits.
- DB crate broad run was intentionally terminated to avoid competing with normal `./dev.py check` (same tests included in its Rust lane); not a passing-suite claim.
- Executive traceability updated under existing REQ-BED-021. No normative guarantee or general recovery subsystem added. Spec-authoring preflight: executive-only status/coverage change, no Allium/wire/type/helper changes; test symbols and requirement anchor checked.

### Exact candidate review and qualification

- Two independent bounded reviewers inspected d6f94afaca8650fff75a638fd63c3e2c9eafc602..8165392f64149abde26f62ae2409f2d6f1d1fe15. No actionable source defect. One reviewer found 93007 already allocated to another retained owner's aggregate-capable task. `taskmd new` allocated 93008; filename-only verification found no conflicting local task. Only this mission's 93007 file removed; original owner trees/IDs unchanged. Actual current task ID is 93008.
- Source migration/helper and service tests unchanged from reviewed 8165392. One introduced check failure: forward-only ledger test still expected latest migration112; updated snapshot to include113, retaining prior entries. Full migration module rerun pending.
- `./dev.py check`: 17/19 steps passed. Rust tests stopped after stale ledger snapshot (172 passed, 1 failed; 2409 not run); that introduced failure corrected. dev.py unit tests: 584 run, one unrelated error in `test_paired_bootstrap_interruption_retains_prepared_claim_without_overwrite`: rollback binary identity mismatch. Reproduced alone with pinned dev Python. dev.py/deploy tests untouched in this diff; record for separate owning stream, do not fix or perform host operations here. All printed host-action lines came from isolated mocked tests, not actual deployment.
- Reviewer noted reduced historical fixture stamps other migrations rather than running a full historical database upgrade/reopen; current full-schema service regression complements it. This limits broad upgrade qualification, not a reproduced source defect. Normal hosted CI and Global's release/deploy acceptance remain required; no additional review committee or redesign.

- Widened migration module exposed two historical partial-schema fixture constructors that deliberately skip receipt schema74/100; stamped113 alongside their existing skipped migrations, without changing production schema handling. After ledger and these fixture-only updates, the entire migration test module passes 78/78. Migration helper, send-chat source and gates remain byte-identical to reviewed8165392. PR #845 published for normal hosted qualification; Global retains merge/deploy ownership. Local full-check launchd fixture remains unrelated blocking qualification evidence, not fixed here.

## Source mission completed; Global handoff

- Qualified source HEAD c3b21a35af4ffe00bca788407ad4baafe7050162: hosted run https://github.com/scottopell/phoenix-ide/actions/runs/37312057236 completed SUCCESS; lane planning, Rust, clippy, e2e, UI/specs, and task validation all SUCCESS. Two main-only issue-management jobs correctly skipped.
- Normal hosted Codex review completed for that exact source HEAD at 2026-10-05T12:57:48.386628Z with no major issues. Full reviews/threads/comments pagination consumed: zero review threads and no nested replies; no outstanding finding. GitHub reports CLEAN/MERGEABLE.
- Final safe main readback940c6e1591a16ae1f30a4d892f78032e3c58bf6f has latest migration112: no113 collision, merge conflict, or seam reason to rebase. Recheck if main advances before merge.
- Local launchd failure is reproduced unchanged on archived parent d6f94afaca8650fff75a638fd63c3e2c9eafc602 and candidate with pinned Python: PreparationTests.test_paired_bootstrap_interruption_retains_prepared_claim_without_overwrite exits1 in _resolve_rollback_identity with staged rollback identity mismatch. Unstubbed health identity read can conflict with fake previous1.0.0/b*40. This is baseline-present local qualification evidence, not a waiver; source and normal hosted checks passed independently. PR comment https://github.com/scottopell/phoenix-ide/pull/845#issuecomment-5994809871 contains exact reproduction classification.
- Task completion commit contains metadata only. Existing source repair, historical regression, negative/atomicity tests, gates, and owner histories remain unchanged. Global owns normal final merge/release decisions; no production activation performed or authorized by this source completion. Neither retained owner is claimed operationally repaired before separately authorized deployment/acceptance.
