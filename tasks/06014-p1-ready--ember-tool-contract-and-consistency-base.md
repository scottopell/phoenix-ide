# Ember phase 1: tool contracts and consistency baseline

Parent: [06013](./06013-p1-ready--ember-consistency-and-durable-tracking.md). Read its accepted user decisions, proposed scope, derisking evidence and non-goals before implementation. This is the first implementation phase.

## Outcome

Ember has an accurate, production-derived capability map and coherent instructions for composing its existing tools. Freeze the new event-evidence and tracking/scanner boundaries before adding stateful behavior.

## Work

- Read specs/global-recall, specs/coordinator-authority, specs/reminders, specs/conversation-goals, specs/compatibility and relevant federation/continuation contracts. Re-ground paths and ownership at implementation time.
- Re-ground against current deployment and main. RC5 already has nested create/rename request schemas and peer create/rename/continue/Goal operations; these are delivered foundations, not missing work. Preserve explicit unsupported operations such as peer rename receipt-read by operation ID alone. The historical error cohort includes negative tests and older routing defects; do not present it as current reliability.
- Generate definitions through RuntimeManager::coordinator_tool_registry, including configured spawn options, language variants and optional authenticated catalog. The measured installed surface had 27 tools including skill, with 41,923 description/schema characters, not tokens. Capture schemas/descriptions/deferred flags and byte/word metrics; do not hand-author a second authoritative tool list.
- Correct verified guidance drift: creation is installed despite ToolRegistry's stale comment; watch descriptions must include supported nonterminal waits.
- Make native-first/public-API-fallback guidance and identity handoffs explicit. Preserve stable product targets versus exact transcript/message/predecessor/summary/run/revision/reference meanings and deliberate local/peer differences.
- Draft tracking requirements/executive and an ADR for singleton-owned memory, retired records, normalized references, explicit source coverage, fresh context retrieval, no new work scheduler and feature-scoped forward recovery. Record the user's roadmap deprecation separately; phase 2 owns its removal.
- Specify occurrence-bound completion evidence and its retention/projection contract for [phase 3](./06020-p1-ready--bind-watch-events-to-ending-response-evi.md). Reuse the terminal obligation's exact response carrier; distinguish execution generation from source-content version. Existing historical/remote facts need honest absent-capability states. Preserve original diagnostics, private/shared boundaries and current event timing. [Phase 4](./06021-p1-ready--enrich-ember-event-context-and-measure-f.md) qualifies the bounded excerpt and compact model projection; do not build another send alert or summary model.
- Freeze capture versus rendering limits before capture implementation: initial evaluation uses 1,024 UTF-8 bytes for both. Persist capture provenance; a later higher rendering limit cannot recover uncaptured content from changed source. Before trials, declare attribution/authority gates, targeted retrieval reduction and allowed total-context/latency regressions in control cases.
- User-selected scanner scope is Ember's own continuation chain ONLY. Capture user-requested work and meaningful conversational bookmarks; assistant context is supporting evidence, not a worker-promise inventory. Read Issue #1075: preserve the stopping point/question and make returning easy without turning curiosity into a task. Use one item per meaningful concern/commitment and no hierarchy. Full-history versus date-bounded initial backfill and automatic attention behavior remain under review. Specify a frozen activation boundary and separate live/backfill frontiers; do not let tail scanning mark a historical gap covered. Establish exact host-owned application/batch identity independently of model semantic/evidence matching. Retained evidence is not a promise that the model reproduces an extraction key. Worker/remote transcripts are excluded from scanner discovery.
- Resolve exact API replay/revision behavior and scanner route/cost limits from evidence before activation. Avoid adding per-item priority/deadlines/assignments by default.
- Keep query_database as a supported narrow evidence path. Event-follow-up latest-message reads are the first measured journey to improve. Reconcile existing search-performance and native Close owners before commissioning additional work; no universal discovery wrapper is required.

## Schema derisking constraint

The previous panel's flat-schema criticism was overconfident. Existing tests deliberately prohibit root oneOf/allOf/anyOf for provider compatibility; deployed create/rename already use nested request unions beneath object roots. OpenAI and Anthropic adapters pass input schemas through. Keep current shapes while correcting descriptions. Only a genuinely new shape requires a small acceptance matrix against actual supported destinations using sanitized inputs; preserve runtime validation. No blind root-union rewrite or repeat qualification project for already-delivered changes.

## Starting files

crates/phoenix-ide/src/runtime.rs::coordinator_tool_registry; coordinator_tools.rs; coordinator_tools/instance.rs; conversation_action_tools.rs and submodules; system_prompt.rs; crates/phoenix-core/src/llm_language.rs; crates/phoenix-tools/src/lib.rs and reminders.rs; crates/phoenix-llm/src/{openai,anthropic}.rs; crates/phoenix-skills/src/{lib.rs,builtin/phoenix-api/}.

Existing task 20005 owns instance-routing architecture; task 28007 owns compact typed event cards. Consume/re-ground those owners and the existing native Close/search investigations rather than creating parallel frameworks or UI projects.

## Done when

Full runtime inventory is reproducible; examples compose actual emitted identities for local read/send/watch, selected-peer query/read/watch, exact generated continuation and operation-specific peer reconciliation. Existing valid inputs and their effects remain unchanged in this slice. Provider schema constraints remain covered. New contracts distinguish exact event evidence, captured memory, authorization, coverage and execution. Relevant spec/registry/provider tests and the supported dev.py lanes pass; live acceptance is reported separately.

Out of scope: rebuilding delivered schemas or federation, broad tool consolidation, event runtime implementation, tracking persistence and scanner inference.
