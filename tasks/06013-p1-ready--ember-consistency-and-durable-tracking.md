# Ember coordinator consistency and durable tracking

## Objective and scope

Make Ember's exposed coordination tools understandable and composable, give the existing Global Coordinator a durable record of what it is keeping track of, add a cheap transcript-scanning safety net that automatically captures omissions, and retire the stale GitHub roadmap. This task owns delivery across the phases below; completion of the planning pass is not implementation completion.

User decisions from the planning conversation:
- "things ember is keeping track of is right"
- "the scanner auto adds, put the burden on ember to remove if it’s truly stale, this would then be visible to next scan"
- "deprecate roadmap as part of this, it’s so stale and infrequently used"
- "Wait there is only one ember global coordinator though"
- Granularity: "1) use recommendation" — one item per independently meaningful concern or commitment, with a resumption/follow-through note and no parent/child task hierarchy initially.
- Discovery scope: "2) ember only, focus is less on promises the agents made and more on user requested work, see adhd related prompt suggestion in gh issues".

Related source: [Issue #1075 — preserve curiosity threads](https://github.com/scottopell/phoenix-ide/issues/1075). Preserve current topic, meaningful earlier threads, decisions, unanswered questions and a useful resumption point. A curiosity/bookmark is distinct from requested work; changing topics neither abandons the old thread nor commissions more tasks. Answer the current question first, avoid repeated inventories and topic-switch permission questions, and give a short refresher when the user returns.

Attention behavior remains under discussion: adding a durable record means retaining a bookmark/request for subsequent context; it does not itself create a timed reminder, subscription or work-dispatch request. The user's question about "saving" asks for clarification, not approval of a new automatic wake policy. Full-history versus an explicit date-bounded initial backfill is also still being reviewed; do not treat a cutoff date as decided or as an expiration policy for records already saved.

There is ONE Ember singleton and ONE tracking list attached to its durable product identity, independent of transcript continuation. Remote Phoenix instances are locations of referenced work, not other owners of this list. No shared-list synchronization is commissioned.

## Observed journey and verified environment

A user requests work or explores a question, then changes topics; work and discussion span messages, continuations, conversations and PRs. The requested work or place to resume can gradually disappear from successive summaries even though the history survives. Watches, reminders and message receipts retain input facts, not an independent record of the user's unfinished work or conversational threads. Agent-made promises are supporting context, not the scanner's primary capture target.

Source baseline: e74290876a587a8c1b795f91d623825f92d0cbde, clean detached checkout before planning; planning branch plan/ember-tracking-consistency. This is source investigation plus disposable local experiments, not deployed behavior qualification.

Three independent review lenses (product, reliability, agent usability) traced the full surface. Full runtime assembly has 26 tools, plus optional authenticated skill, rather than only the 15-tool base registry fixture. See RuntimeManager::coordinator_tool_registry in crates/phoenix-ide/src/runtime.rs, coordinator_tools::tools, conversation_action_tools::tools, coordinator_tools/{goals,authority}.rs, reminder_service::tool, and ToolRegistry::coordinator in crates/phoenix-tools/src/lib.rs.

Roadmap Issue #806 was read through GitHub on 2026-11-14: its generated projection was rendered that day, while many recorded outcomes still showed weeks-old check-ins. Render freshness is not outcome freshness. No records, issue content, variables, workflows or production state were changed.

## Proposed product contract

### Keep the existing authorities distinct

| Concept | Responsibility |
| --- | --- |
| Tracking item | User-requested work or a meaningful conversational bookmark, why it matters and where to resume |
| ProductConversation / transcript | Durable execution identity / exact historical evidence |
| Goal | One worker conversation's objective and disposition |
| Authority | Saved standing instructions and scope grants |
| Watch | Future conversation events; supported waits as well as terminal facts |
| Reminder | One-shot ordinary input at a scheduled time |
| Task / PR / deployment | Repository work status / review state / deployed evidence |

Tracking does not mirror PR or worker runtime status, issue commands, create Goals, grant permission, or introduce another execution scheduler. A successful send or accepted event does not resolve an item. The roadmap is retired rather than mirrored or dual-written.

### Record and native interface

Proposed record: server-owned stable ID, title, prose note explaining purpose/current understanding, resumption/follow-through (next useful question, action or watch condition), tracking/resolved/dismissed disposition, required nonblank retirement reason when retired, revision and integer microsecond creation/update/review times. Preserve the distinction between a curiosity bookmark and explicitly requested work; phase 1 chooses the smallest representation that keeps bookmarks from becoming execution obligations. This distinction is not a new permission grant. Last-reviewed time must mean an explicit review, not merely a successful scan.

Use child rows for typed references and originating evidence, with explicit ordinal. Phoenix locators include immutable instance identity, product identity for a current target, and exact transcript/message identity for historical evidence. PR locators identify repository and PR; no cached PR-state authority is introduced. References that outlive source deletion remain honest unavailable evidence rather than fabricating content. Decide their feature-scoped retention in the normative contract.

Store source references and author/origin facts; scanner interpretations and prose do not become new user authorization. Avoid a mandatory model-inferred "user commitment" enum as a substitute for primary evidence.

Expose one compact tracking capability with bounded listing/reading, recording/updating and explicit retirement/reopening through one application service. Exact operation names and provider-safe shape are established in the contract phase. Scanner writes use the same service. Return committed identity/revision and conflict state. Mutations use trusted invocation identity and revision fences; freeze the feature's precise replay contract before choosing operation IDs. Do not copy the entire Goal prepare/apply framework by analogy.

Explicit Ember decisions may reopen retired work on new evidence. Scanner may add evidence to an identified existing item; it may not silently reopen, retire, overwrite Ember's notes, or strengthen a speculative idea into action authority. Duplicate captures can be explicitly dismissed with a reason rather than requiring a new merge subsystem.

### Continuity and visibility

Use the existing fresh Coordinator-context path to include a bounded, server-rendered tracking projection at dispatch, including after continuation: registry revision, active count, entries/pointers, and explicit pagination/coverage. Larger registries must never look fully reviewed because only a prefix was supplied. Full notes/evidence remain available through the tracking tool.

This is an intentional new durable-memory projection, not a resurrection of the removed inferred activity capsule. Update global-recall contracts/ADRs accordingly. Stored note text is untrusted data; the server owns identity/context framing, not the meaning or authority of prose.

Keep /global chat-first. Establish what users can inspect through conversation/tool results first. If a bounded list/detail UI is needed, specify it deliberately, with web/native parity or explicit surface scope; do not quietly add a workstream dashboard, hierarchy, or new navigation identity. Tracking persistence and retrieval must not depend on a UI being mounted.

### Scanner and coverage

User-selected discovery scope: ONLY the existing Ember continuation chain, incrementally over persisted user/assistant prose with preserved source origins. Capture user-requested work and meaningful unfinished conversational threads; assistant prose supplies context, decisions and stopping points rather than making agent promises the primary target. Exclude raw tool dumps and worker/inline-child transcripts from scanner discovery. Ember may still use existing tools to inspect linked workers while following up on a user request; that does not enroll those transcripts in discovery.

Recommended historical scope is the whole singleton chain in bounded batches. The user is considering full history versus a date-bounded initial backfill; the exact start point remains a product decision. An optional lower date bound limits initial discovery only, never expires older tracked records, and must leave the excluded history explicitly unscanned. Freeze an activation high-water boundary and maintain separate live and historical frontiers for each preexisting transcript; a new tail may advance without marking an older gap covered. New continuation members enter live coverage from their start. Use one bounded worker with fair lane selection, not a general scheduler. Old roadmap excerpts, quoted prompts and historical claims are evidence, never fresh commissions. Automatic historical capture may temporarily retain an already-stale suggestion whose later completion/cancellation has not yet been scanned. Preserve historical provenance and state freshness as unverified; Ember must inspect later/current evidence before acting. A bookmark may remain useful without requiring action at all. Prioritize new persisted Ember content; historical backfill must not monopolize the worker. Resolve start-point/date semantics and coverage UX in phase 1 before implementing automatic capture.

Use one supervised bounded background inference service, with durable per-transcript coverage/cursors, coalesced post-commit triggers and startup discovery. A missed trigger is recovered by querying uncovered committed content. Do not create an ordinary agent conversation, a Goal, or a general-purpose job scheduler for scans.

Each input snapshot binds transcript generation, sequence range, source message evidence and current tracking-registry revision. Oversized messages require a generation/digest-bound fragment position or explicit unprocessed status; a scanned prefix must never advance coverage beyond the whole message. transcript_generation does NOT increment on append; an incremental scanner needs generation AND sequence. Edits/recovery invalidate affected old-generation coverage. A complete valid empty result may advance its exact scanned range; failed, truncated, skipped or invalid inference may not.

Define host-owned batch/application identity and source locators separately from model semantic matching; never depend on the model reproducing an extraction key. Apply validated captures/evidence and coverage atomically. Registry/source/cursor compare-and-swap rejects a result that raced an Ember edit/dismissal or source rewrite, without advancing coverage. Re-read and recompute from current active and relevant retired records. Do not await inference in direct_turn_worker::discover_inputs or the foreground admission path. One in-flight local batch is sufficient initially; no exactly-once model-call or perfect semantic-dedup guarantee is implied. Replayed committed ranges cannot repeat application, and existing retired records cannot be reopened by the scanner; a differently expressed new duplicate record remains possible and is measured separately. Model calls can repeat after interruption; committed mutations must not repeat.

Explicit model route, bounded input/output/time/retry and backfill budgets, and visible last-success/failure/coverage are required. Current get_cheap_model_with_id falls back to the registry default; do not silently turn the scanner into expensive foreground-model work. No eligible route means visible pending/degraded scanning while direct tracking and normal Ember work continue.

## Full exposed surface and consistency boundary

| Family | Tools covered |
| --- | --- |
| Reasoning/delegation | think, spawn_agents, optional skill |
| Evidence/navigation | search_conversations, read_conversation, query_database, resolve_reference |
| Execution/presentation | bash, work_scope_load, present_svg |
| Coordination | send_conversation_message, ask_user_question |
| Events/time | watch_conversation, unwatch_conversation, list_watched_conversations, reminders |
| Conversation lifecycle | create_conversation, rename_conversation, continue_conversation, read_automatic_continuation, set_automatic_continuation, retry_close_safely |
| Goal metadata | read_goal, prepare_goal, apply_goal |
| Standing scope | read_authority, set_authority |

Preserve semantic differences between stable targets, exact transcripts, generated summaries, run ordinals, revisions and prepared references. Improve copyable outputs and supported handoff examples rather than replacing all with an ambiguous target string.

Preserve explicit remote routing and no local fallback. Search remains local; read/query/resolve/send/watch and automatic-continuation settings support selected peers; Goal reads support peers while Goal writes and create/rename/continue remain local. The consistency pass is not universal federation.

## Implementation phases and dependencies

1. [Contract and surface baseline — 06014](./06014-p1-ready--ember-tool-contract-and-consistency-base.md): draft requirements/ADRs, full production-constructor inventory, provider constraints, identity journeys, native-first guidance, concrete source-coverage decision, and scanner evaluation cases. Repair verified wording drift without changing call shapes. This phase precedes new stateful behavior.
2. [Roadmap retirement — 06015](./06015-p1-ready--retire-stale-delivery-roadmap.md): supersede ADR-069, mark the entire retained REQ-ROADMAP-001–014 protocol explicitly historical and non-normative, preserving IDs/original text, update executive and active agent instructions, quiesce/remove automation and preserve issue evidence. Independent of scanner delivery; no dual writing.
3. [Tracking domain/storage/application service/native tools — 06016](./06016-p1-ready--ember-durable-tracking-store-and-tools.md): normalized schema, retention, revision/replay contracts, dispositions, exact evidence and references; no background model calls yet.
4. [Continuity and inspectability — 06017](./06017-p1-ready--ember-tracking-continuity-and-inspection.md): fresh bounded Coordinator projection, continuation/restart retrieval, user inspection, honest partial coverage, source-navigation behavior and isolation from ordinary/child registries.
5. [Scanner — 06018](./06018-p1-ready--ember-cheap-transcript-safety-net.md): bounded inference, persisted coverage and retry state, source-enrollment/backfill policy, atomic CAS reconciliation, cheap-route limits and shadow qualification followed by automatic capture.
6. [End-to-end qualification and cutover — 06019](./06019-p1-ready--ember-coordination-end-to-end-qualificat.md): full-registry scenarios, scanner adversarial evaluation, migration/restart/continuation tests, existing notification/Goal/Authority boundaries, live controlled acceptance and selective reconciliation of still-live historical obligations.

Dependencies: 06014 precedes new stateful behavior; 06015 is independently shippable; 06016 follows 06014; 06017 follows 06016; 06018 follows 06014/06016/06017; 06019 qualifies the delivered slices. Task filenames remain ready because these are planned dependencies, not claims that predecessor work is already implemented.

Each phase has its own child task. Use logical PRs; preserve explicit dependencies and never describe a foundation PR as the entire feature. Do not wait for scanner availability to stop stale roadmap automation.

## Roadmap retirement cutover

Executable surface: .github/workflows/roadmap-issue-reducer.yml; scripts/roadmap-issue-reducer.mjs and its dedicated test; the CI fast-lane invocation in .github/workflows/ci.yml. No roadmap dependencies were found in Rust runtime, UI, iOS or dev.py.

Active operating instructions live in AGENTS.md and canonical skills/phoenix-development/SKILL.md; .agents/skills is an alias and CLAUDE.md imports AGENTS.md. Review active tasks that commission roadmap updates, especially 17001, 44013, 45015, 06002, 98020, 46005, 33001, 68002 and 58059. Preserve historical incident/commission links and unrelated task scope.

Closing #806 does not disable the reducer: run() checks variables and activation marker, not issue state. Live order: inspect current workflow/runs/config/body; disable and quiesce updater runs; clear roadmap variables; remove activation marker and add historical-only notice while retaining content/comments; verify stable archived evidence. Do not delete historical records or blindly import roadmap rows. Issue closure is optional, not the shutdown mechanism.

Code review/merge and live GitHub cutoff are separately observable deliveries. Do not claim remote automation is stopped merely because the workflow was deleted on a branch. New ADRs explain deprecation. Preserve REQ IDs and original EARS text for traceability, with an explicit whole-document statement that the retained protocol is historical/non-normative and no retired SHALL is an active obligation. A deprecated heading alone is insufficient.

## Derisking performed on 2026-11-14

- Three independent source investigations confirmed the complete registry, actual routing asymmetry, API guidance expansion, roadmap retirement blast radius, transcript-generation semantics and cheap-model fallback.
- Corrected an earlier panel overstatement: flat op-based schemas are deliberate provider-compatibility work, not by themselves a defect. Existing tests in coordinator_tools.rs, reminders.rs and browser/tests.rs forbid root oneOf/allOf/anyOf. Provider adapters pass schemas through unchanged. Do not replace roots with unions on the strength of local JSON Schema validation.
- Official provider documentation also distinguishes JSON Schema support and strict-mode subsets. A live provider acceptance test remains necessary for any proposed new shape. Existing strict mode and endpoint defaults must be measured rather than inferred.
- API skill expansion source baseline: 4,097 whitespace words / 32,070 bytes across SKILL.md and its appended API reference. Splitting companion files alone does not reduce expansion because the loader appends all companions. These are source word/byte counts, not tokens or full installed-schema size.
- Ran a disposable Node 26 / SQLite 3.53.4 prototype against a new ignored scratch DB. Twelve checks exercised atomic rollback, incomplete-output coverage, successful record/cursor commit, stale-snapshot replay, already-covered ranges, concurrent dismissal, exact fixture extraction-key suppression, source-generation conflicts/rescan, mandatory retirement reason, database reopen, and a deliberate semantic-duplicate counterexample.
- Prototype initially exposed SQLite CHECK NULL semantics: length(trim(retirement_reason)) > 0 alone permits NULL. Fixed the sketch by requiring retirement_reason IS NOT NULL for retired states. Production schema must test this explicitly.
- The duplicate counterexample succeeds in adding a second meaning-equivalent item when extraction keys differ. The prototype establishes NO semantic identity guarantee, provider behavior, Rust migration correctness or production runtime recovery.
- Scratch evidence: .phoenix/ember-planning/reconcile-spike.mjs (ignored, machine-local), SHA256 c609693e9d3fe5b45c6f3b6694289ce0e29bfc0babc3756397bd4485f57e52e0. It is planning evidence, not a required hidden dependency for implementation. Recreate the scenarios as production tests in the owning phases.
- A second disposable SQLite frontier prototype passed six checks for frozen activation boundaries, new-tail progress during same-transcript backfill, an explicit unscanned gap, bounded backfill, partial oversized-message coverage and source-generation invalidation. It does not exercise real canonical transcript selection/quote validation. The first transaction prototype has one cursor and does not prove this two-frontier policy; the separate prototype isolates that design risk.
- Existing ./dev.py qa coordinator-drive-eval uses four fake tools and archived prompt variants; it cannot qualify full registry, real SQL discovery, watches, lifecycle actions or actual continuation. MODE=prepare can build; do not mislabel it as a no-build check.

## Validation matrix and completion evidence

| Journey | Required evidence |
| --- | --- |
| Complete tool surface | Generated full-runtime definitions, both prompt languages, skill present/absent, configured spawn choices; provider serialization |
| Identity composition | Local find/read/send/watch, peer query/read/watch, exact generated continuation, operation-specific reconciliation |
| Direct tracking | Commit/retry/conflict, two items in one message, malformed refs, retired reason, origin evidence, missing source handling |
| Continuity | Same item identity and disposition across continuation and runtime reopen; source citation stays exact while current target follows successor |
| Scanner | Repeated range, interrupted apply, source edit, dismissal race, quoted idea, superseded request, changed wording, two distinct commitments, partial output, model unavailable, token limits, new tail during incomplete same-transcript backfill, and later cancellation outside the historical chunk |
| No authority escalation | Captured historical text/peer/tool/child output cannot become user authority; no model write beyond tracking service |
| No accidental scheduler | Tracking/scan completion does not dispatch worker work, create Goals, rearm stopped work or redeliver accepted inputs |
| Roadmap | No active posting instructions or executable updater; old evidence readable; live workflow/runs/config/body readback |
| Human utility | Real continuation returns the owed next step without user reminding; capture noise and duplicate load reviewed by user |

Use ./dev.py check-plan to select relevant lanes. Planned repository checks: task validation; spec-shape/spec-anchors and allium where specifications warrant it; Rust/fmt/clippy for runtime changes; UI/native checks only for the scoped presentation changes. Hosted/native resources must be admitted by existing owners; no heavy local build was run for this planning pass.

Schema live acceptance and cheap-model recall/duplicate/cost/latency evaluation remain gates, not claimed successes. Use synthetic sanitized cases and real production constructors; record model ID, endpoint, prompt/schema hashes, token usage, source coverage and failure category. Do not use a model to grade its own omission recall as sole evidence.

## Complexity limits and existing ownership

Do not add shared registries, cross-instance synchronization, full-corpus remote scanning, a task/project hierarchy, PR/GitHub mirroring, a generic job scheduler, semantic exactly-once promises, automatic roadmap import, a universal mutation wrapper, or one native tool per HTTP endpoint.

Use existing instance-routing work (task 20005) rather than commissioning another routing framework. Preserve the existing conversation identity/lifecycle program (task 92009): tracked things are obligations spanning execution conversations, never a replacement identity or a second conversation state machine.

Allium is justified for scanner snapshot/apply/coverage and retirement concurrency if those contracts remain multi-step; simple CRUD does not independently require it. Define forward migration, evidence-retention and scanner recovery guarantees explicitly under specs/compatibility/requirements.md. No downgrade, live DB replacement, cross-version replay or perfect model recall guarantee is added by implication.

## External technical references

- [OpenAI function calling](https://developers.openai.com/api/docs/guides/function-calling)
- [Claude strict tool use](https://platform.claude.com/docs/en/agents-and-tools/tool-use/strict-tool-use)

Provider documentation guides the acceptance spike; checked-in tests and actual destination behavior remain separate evidence.
