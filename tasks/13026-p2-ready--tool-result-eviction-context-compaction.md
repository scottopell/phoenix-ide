# Tool-result eviction (in-request context compaction)

Long agent conversations accumulate large tool-result payloads (file reads,
bash output, search results) whose value decays sharply once the agent has
acted on them. Evicting old tool-result *payloads* from the LLM request —
replacing them with a short placeholder — would extend how far a single
conversation runs before hitting the 90% continuation threshold
(`CONTINUATION_THRESHOLD`, `should_trigger_continuation` in
`state_machine/transition.rs`).

This is a deliberate cross-provider optimization, distinct from the forced
provider-capability drop in task 13021.

## Chokepoint
`build_llm_messages_static` (`crates/phoenix-ide/src/runtime/executor.rs:2185`)
is the single place stored DB rows are assembled into `Vec<LlmMessage>`.
Eviction belongs here, or in a pass applied to its output before the
provider `translate_*` step.

## Hard constraints
- **Replace, never delete.** A `tool_use` with no matching `tool_result` is an
  Anthropic 400. Eviction must keep the `ToolResult` block and its
  `tool_use_id`, swapping only the `content` (and `images`) for a short
  placeholder, e.g. `[stale tool result elided — N bytes]`.
- **Caching tension — evict in batches, not per turn.** Both providers cache on
  the message prefix. Evicting a result mid-history changes the prefix bytes
  and busts the cache from that point onward. Per-turn incremental eviction
  busts the prefix every turn and re-pays it — usually a net loss. Viable
  shape: occasional batched eviction at a context-pressure threshold, so the
  one-time re-cache cost amortizes over many subsequent cached turns. Rule of
  thumb: worth it when
  `reclaimed_tokens × turns_until_next_eviction > one_cache_bust`.

## Caching model (from the caching exploration)
- **Anthropic** — manual breakpoints: system, last tool, last content block of
  last user message (`anthropic.rs:515-595`). Evicting content *before* the
  last-user-message breakpoint busts that cached prefix. NOTE: the message
  breakpoint is currently dropped during tool loops — see the sibling p1 task
  "Anthropic message-history cache breakpoint silently dropped during tool
  loops". Design eviction *after* that fix lands so the Anthropic cache model
  is consistent.
- **OpenAI** — automatic prefix caching via `prompt_cache_key` (= conversation
  id, `openai.rs:726`). Eviction busts the automatic cache from the eviction
  point regardless.

## Design decisions to resolve (settle these in the spec — do not guess)
1. **View-only vs persisted.** Recommendation: apply eviction only at
   request-build time in `build_llm_messages_static`; the DB keeps the full
   tool-result record (crash recovery, UI display, audit). Eviction is a
   per-request context-budget view, not a destructive data change — and this
   avoids a migration.
2. **Trigger threshold.** A new threshold below the 90% continuation trigger
   (e.g. 60-70% of the context window) so eviction buys runway before
   continuation is needed.
3. **What to evict.** Age- or token-budget-based: keep the last N tool rounds
   verbatim, evict older ones. Decide N or the keep-budget.
4. **Batch granularity.** Evict a large chunk at once to amortize the cache
   bust; do not evict incrementally per turn.
5. **Images.** Tool-result images are token-heavy — evicting image payloads is
   high value; include them.
6. **Relationship to continuation.** Eviction is a gentler, earlier lever than
   the REQ-BED-019 continuation flow. Decide whether it complements the 90%
   trigger (continuation still fires as a backstop) or shifts it.

## Spec
This is a behavioral change to context management — write a spEARS spec under
`specs/` (REQ-BED area) capturing the trigger, eviction policy, placeholder
contract, and caching rationale before implementing. An Allium spec is likely
warranted: a threshold-driven lifecycle where ordering matters.

## Dependency
Design and implement after the sibling p1 task (Anthropic tool-result cache
breakpoint) so the Anthropic cache model is settled first.
