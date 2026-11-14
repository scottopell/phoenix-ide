# Ember phase 1: tool contracts and consistency baseline

Parent: [06013](./06013-p1-ready--ember-consistency-and-durable-tracking.md). Read its accepted user decisions, proposed scope, derisking evidence and non-goals before implementation. This is the first implementation phase.

## Outcome

Ember has an accurate, production-derived capability map and coherent instructions for composing its existing tools. Freeze the new tracking/scanner boundaries before adding stateful behavior.

## Work

- Read specs/global-recall, specs/coordinator-authority, specs/reminders, specs/conversation-goals, specs/compatibility and relevant federation/continuation contracts. Re-ground paths and ownership at implementation time.
- Generate definitions through RuntimeManager::coordinator_tool_registry, including configured spawn options, language variants and optional authenticated catalog. Capture schemas/descriptions/deferred flags and byte/word metrics; do not hand-author a second authoritative tool list.
- Correct verified guidance drift: creation is installed despite ToolRegistry's stale comment; watch descriptions must include supported nonterminal waits.
- Make native-first/public-API-fallback guidance and identity handoffs explicit. Preserve stable product targets versus exact transcript/message/predecessor/summary/run/revision/reference meanings and deliberate local/peer differences.
- Draft tracking requirements/executive and an ADR for singleton-owned memory, retired records, normalized references, explicit source coverage, fresh context retrieval, no new work scheduler and feature-scoped forward recovery. Record the user's roadmap deprecation separately; phase 2 owns its removal.
- User-selected scanner scope is Ember's own continuation chain ONLY. Capture user-requested work and meaningful conversational bookmarks; assistant context is supporting evidence, not a worker-promise inventory. Read Issue #1075: preserve the stopping point/question and make returning easy without turning curiosity into a task. Use one item per meaningful concern/commitment and no hierarchy. Full-history versus date-bounded initial backfill and automatic attention behavior remain under review. Specify a frozen activation boundary and separate live/backfill frontiers; do not let tail scanning mark a historical gap covered. Establish exact host-owned application/batch identity independently of model semantic/evidence matching. Retained evidence is not a promise that the model reproduces an extraction key. Worker/remote transcripts are excluded from scanner discovery.
- Resolve exact API replay/revision behavior and scanner route/cost limits from evidence before activation. Avoid adding per-item priority/deadlines/assignments by default.

## Schema derisking constraint

The previous panel's flat-schema criticism was overconfident. Existing tests deliberately prohibit root oneOf/allOf/anyOf for provider compatibility. OpenAI and Anthropic adapters pass input schemas through. Keep object roots and current call shapes while correcting descriptions. If a nested envelope/conditional improvement is proposed, first run a small acceptance matrix against actual supported destinations using sanitized inputs; preserve runtime validation. No blind root-union rewrite.

## Starting files

crates/phoenix-ide/src/runtime.rs::coordinator_tool_registry; coordinator_tools.rs; coordinator_tools/instance.rs; conversation_action_tools.rs and submodules; system_prompt.rs; crates/phoenix-core/src/llm_language.rs; crates/phoenix-tools/src/lib.rs and reminders.rs; crates/phoenix-llm/src/{openai,anthropic}.rs; crates/phoenix-skills/src/{lib.rs,builtin/phoenix-api/}.

Existing task 20005 owns instance-routing architecture. Consume/re-ground that work rather than creating another framework.

## Done when

Full runtime inventory is reproducible; examples compose actual emitted identities for local read/send/watch, selected-peer query/read/watch, and exact generated continuation. Existing valid inputs and their effects remain unchanged in this slice. Provider schema constraints are tested. New memory/scanner requirements distinguish captured evidence, authorization, coverage and execution. Relevant spec/registry/provider tests and the supported dev.py lanes pass; live acceptance is reported separately.
