# Ember phase 1: tool contracts and consistency baseline

Parent: [06013](./06013-p1-in-progress--ember-consistency-and-durable-tracking.md). Read its accepted user decisions, proposed scope, derisking evidence and non-goals before implementation. This is a shared baseline task, not a first-complete global implementation gate. Lanes A/B own their necessary contract subsets; lane C closes the remaining bounded audit after roadmap retirement.

## Outcome

Ember has an accurate, production-derived capability map and coherent instructions for composing its existing tools. Keep shared terminology consistent, with feature-local contracts accepted before the behavior they govern. This task is not a whole-program design freeze.

## Feature-local prerequisites

- Event capture/projection needs only its relevant baseline and occurrence/retention/privacy/capture/follow-up-read contract. Accepting that subset unblocks phases 3–4 even while tracking/scanner work in this task remains unfinished.
- Tracking persistence needs its record/native-tool/reconciliation contract before phase 5 implementation; event and scanner details do not gate it.
- Scanner frontier, generation invalidation, application identity, route and budget decisions belong to phase 7 and must be settled before scanner activation. They do not block event or direct-tracking delivery.

The owning feature task specifies and qualifies its contract. This shared task keeps the map and guidance aligned without duplicating normative authorities or adding another phase.

## Work

- Read specs/global-recall, specs/coordinator-authority, specs/reminders, specs/conversation-goals, specs/compatibility and relevant federation/continuation contracts. Re-ground paths and ownership at implementation time.
- Re-ground against current deployment and main. RC5 already has nested create/rename request schemas and peer create/rename/continue/Goal operations; these are delivered foundations, not missing work. Preserve explicit unsupported operations such as peer rename receipt-read by operation ID alone. The historical error cohort includes negative tests and older routing defects; do not present it as current reliability.
- Generate definitions through RuntimeManager::coordinator_tool_registry, including configured spawn options, language variants and optional authenticated catalog. The measured installed surface had 27 tools including skill, with 41,923 description/schema characters, not tokens. Capture schemas/descriptions/deferred flags and byte/word metrics; do not hand-author a second authoritative tool list.
- Correct verified guidance drift: creation is installed despite ToolRegistry's stale comment; watch descriptions must include supported nonterminal waits.
- Retain the production-grounded list-watch regression below, using the closed request contract merged in #1088. That narrow implementation is complete; do not rebuild it as audit work. Inventory coverage alone does not qualify integrated model recovery.
- Make native-first/public-API-fallback guidance and identity handoffs explicit. Preserve stable product targets versus exact transcript/message/predecessor/summary/run/revision/reference meanings and deliberate local/peer differences.
- Align tracking terminology with phase 5's requirements/executive and ADR for singleton-owned memory, retired records, normalized references, review-needed evidence, fresh context retrieval and feature-scoped forward recovery. Phase 5 owns its record/native-tool contract; phase 7 owns precise coverage semantics. Record the user's roadmap deprecation separately; phase 2 owns its removal.
- Specify occurrence-bound completion evidence and its retention/projection contract for [phase 3](./06020-p1-in-progress--bind-watch-events-to-ending-response-evi.md). Reuse the terminal obligation's exact response carrier; distinguish execution generation from source-content version. Existing historical/remote facts need honest absent-capability states. Preserve original diagnostics, private/shared boundaries and current event timing. [Phase 4](./06021-p1-in-progress--enrich-ember-event-context-and-measure-f.md) qualifies the bounded excerpt and compact model projection; do not build another send alert or summary model.
- Freeze capture versus rendering limits before capture implementation: initial evaluation uses 1,024 UTF-8 bytes for both. Persist capture provenance; a later higher rendering limit cannot recover uncaptured content from changed source. Before trials, declare attribution/authority gates, targeted retrieval reduction and allowed total-context/latency regressions in control cases.
- Record the settled scanner scope in the shared map: Ember's own continuation chain ONLY, user-requested work and meaningful bookmarks, all-history bounded backfill prioritizing new messages, and no new periodic review wakeups. Read Issue #1075. Phase 7 owns the live/backfill frontier, generation invalidation, application identity and route/budget contracts; resolve them there before activation. Worker/remote transcripts remain excluded.
- Each feature freezes its own exact API replay/revision behavior before implementing its mutations. Avoid adding per-item priority/deadlines/assignments by default.
- Keep query_database as a supported narrow evidence path. Event-follow-up latest-message reads are the first measured journey to improve. Reconcile existing search-performance and native Close owners before commissioning additional work; no universal discovery wrapper is required.

## Schema derisking constraint

The previous panel's flat-schema criticism was overconfident. Existing tests deliberately prohibit root oneOf/allOf/anyOf for provider compatibility; deployed create/rename already use nested request unions beneath object roots. OpenAI and Anthropic adapters pass input schemas through. Keep current shapes while correcting descriptions. Only a genuinely new shape requires a small acceptance matrix against actual supported destinations using sanitized inputs; preserve runtime validation. No blind root-union rewrite or repeat qualification project for already-delivered changes.

## Concrete regression: list_watched_conversations route and after — hotfix merged

[PR #1088](https://github.com/scottopell/phoenix-ide/pull/1088) merged on 2026-11-15 as 4f275b714b1495ec10f953a33156e3adc1381a59. It delivered closed local/remote schema branches, typed decoding and actionable recovery examples, with five focused Rust and 17 schema cases passing. This is completed source work, not proof of production deployment or live-model acceptance. Phase 1 retains the regression and integrated qualification rather than commissioning the same repair again.

Read-only inspection on 2026-11-15 verified RC5 at 4da5f81c and the installed definition. In the inspected Ember transcript's six list-watch calls, all four local calls supplied after and failed with “unknown field `after`, there are no fields.” Three consecutive attempts changed only the UUID. One selected-peer listing succeeded; another failed separately during peer lookup. Publish only this sanitized sequence, not production message/call/peer identifiers.

The inspected RC5 schema advertised optional after as a UUID in one shared object, with a prose remote-only restriction. Local routing stripped instance, then decoded an empty deny_unknown_fields struct, rejecting after even though the common schema admitted that shape. The raw serde error did not explain the route-specific rule or show recovery. The adapter passed through the schema; this observation does not prove a provider forced the field or explain the model's internal reason for supplying it.

after means a remote subscription-list pagination position, not time, event occurrence or admitted delivery cursor. The merged contract uses {"request":{"op":"local"}} for active local watches, with no peer or cursor fields. Remote requests use {"request":{"op":"remote","instance":"<saved peer UUID or name>"}}, adding after only for paging; omitted/null selects the first page and returned next_cursor continues while has_more is true. Remote output retains lifecycle state, UUID ordering, a 50-entry cap and the byte budget. The closed operation union remains beneath an object root.

Carry forward #1088's local/remote schema agreement, local error recovery, remote null/omitted first page, returned-cursor paging, byte limits, peer isolation and offline unsubscribe coverage. Integrated qualification still needs a composed model-recovery journey: after an invalid historical flat/local-cursor call, use the new valid request rather than guessing more UUIDs. Provider/model and deployed acceptance must be reported separately; unit rejection alone does not establish agent usability.

## Starting files

crates/phoenix-ide/src/runtime.rs::coordinator_tool_registry; coordinator_tools.rs; coordinator_tools/instance.rs; conversation_action_tools.rs and submodules; system_prompt.rs; crates/phoenix-core/src/llm_language.rs; crates/phoenix-tools/src/lib.rs and reminders.rs; crates/phoenix-llm/src/{openai,anthropic}.rs; crates/phoenix-skills/src/{lib.rs,builtin/phoenix-api/}.

Existing task 20005 owns instance-routing architecture; task 28007 owns compact typed event cards. Consume/re-ground those owners and the existing native Close/search investigations rather than creating parallel frameworks or UI projects.

## Done when

Full runtime inventory is reproducible; examples compose actual emitted identities for local read/send/watch, selected-peer query/read/watch, exact generated continuation and operation-specific peer reconciliation. Existing valid inputs and their effects remain unchanged in this slice. Provider schema constraints remain covered. Shared guidance distinguishes exact event evidence, captured memory, authorization, coverage and execution, with contracts owned and gated by their respective feature slices. Relevant spec/registry/provider tests and the supported dev.py lanes pass; live acceptance is reported separately. This task's aggregate completion is not a prerequisite for accepting the event-specific subset.

Out of scope: rebuilding delivered schemas or federation, broad tool consolidation, event runtime implementation, tracking persistence and scanner inference.

## Narrow implementation — 2026-11-16

Corrected the registry composition comment that incorrectly excluded creation,
and the installed watch description to distinguish local nonterminal question/
task-approval waits from selected-peer v1 terminal-only facts. Current Coordinator
instructions already prefer installed native actions and bind public API fallback
to inspected destination contracts; no redundant system-prompt layer was added.

Added an opt-in synthetic inventory export through the production runtime
constructor plus production SpawnCatalog specialization. It records actual
schemas/descriptions/deferred flags, both languages, authenticated catalog on/off,
and generic/configured worker/tier variants, with bytes/words rather than a token
claim. It has no fixed list/count. Final execution/qualification follows memory
integration so new tools cannot be omitted from the reported current surface.

Runtime compilation, eight-variant export readback, the ordinary inventory matrix,
real allowed local-read fixture, registered watch-list and Coordinator prompt
regressions pass. One prepared, bounded Astra/high trial through the normal
discovered Codex route recovered the exact local request on its first tool call,
received the actual enrolled watch and reported its idle state correctly. The
integrator independently accepted this narrow gate. Sanitized trace/hash receipts
and limitations are in scripts/qa/ember-watch-recovery/README.md. This proves one
synthetic model read-selection/registered-dispatch case, not full runtime admission
or production activation. Exact final-head full CI remains required.

**Mandatory final-composition follow-up:** rerun the assembled inventory after
tracking/event integration, reconcile their tool/guidance entries and preserve
actual configuration/language/catalog scope. This remains owned by this task and
[combined acceptance](./06019-p1-in-progress--ember-coordination-end-to-end-qualificat.md).
Do not mark phase 1 complete from the pre-integration inventory or this one trial.
