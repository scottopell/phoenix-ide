# Make model effort a first-class, visible per-conversation setting

## Why

Phoenix currently lets users choose a model but neither exposes nor controls the model's effort/reasoning setting. Every normal provider request omits effort, so the effective behavior is delegated invisibly to the selected model and route. This affects answer quality, latency, tool use, reasoning/output token consumption, and cost.

The current omission is not accidentally selecting one extreme, but it is inconsistent across Phoenix's built-in models:

- Anthropic documents omission as `high` for effort-capable models such as Claude Opus 4.6–4.8, Sonnet 4.6, and Sonnet 5. Phoenix's Anthropic wire type has no `output_config.effort`, so these use `high` today.
- Claude Haiku 4.5 is not listed as supporting effort; Phoenix must not label it as implicitly `high` or offer an unsupported selector.
- OpenAI documents GPT-5.6 and GPT-5.5 omission as `medium` in standard reasoning mode.
- OpenAI documents GPT-5.4 and GPT-5.4 Mini omission as `none`.
- GPT-5.3-Codex remains a built-in, recommended model and a fallback preference despite no longer being wanted. Remove it from Phoenix's built-in catalog rather than extending its capability contract; historical conversations keep their stored model ID, while new selection cannot choose it.
- External Anthropic/OpenAI-compatible models have operator-defined semantics and likewise require explicit capability metadata or an honest unknown state.

Repository evidence:

- `phoenix_core::domain::llm_types::LlmRequest` carries only system, messages, tools, `max_tokens`, telemetry, and cache key; it cannot represent effort.
- `anthropic::AnthropicRequest` omits both `output_config.effort` and thinking configuration.
- `openai::ResponsesApiRequest` has no reasoning configuration. GPT-5.6 Codex Responses Lite sends only `reasoning.context = all_turns`; this is continuity/replay policy, not reasoning effort.
- `ModelInfo`, `CreateConversationRequest`, `UpgradeModelRequest`, the conversations schema, and both model pickers carry only the model ID/metadata.
- Normal conversation turns set `max_tokens: Some(16_384)`. Anthropic recommends substantially more output headroom for Opus at `xhigh`/`max`, so effort and output-limit compatibility must be considered together rather than blindly adding one JSON field.
- OpenAI usage parsing retains aggregate output tokens but not `output_tokens_details.reasoning_tokens`, limiting the user's ability to explain effort-related spend.

## Product decisions

1. **Model-native default:** when the user has not overridden effort, preserve the provider/model/route's native behavior rather than imposing one Phoenix-wide effort.
2. **Per-conversation control:** effort is selected at conversation creation and may be changed while the conversation is otherwise eligible for a model change. The effective setting is visible beside the model in the state bar.
3. **Explicit override versus native default are distinct states:** persistence and APIs must distinguish “follow native default” from an explicit value that happens to equal today's native default. This prevents provider default changes from being silently pinned and supports honest UI labels such as `High (model default)`.
4. **Model switch:** if an explicit override is unsupported by the target model, switching succeeds and atomically resets effort to the target model's native default. Do not silently coerce to a nearby effort.
5. **Sub-agents:** each spawned conversation uses its own selected model's native default. A parent conversation's override is not inherited.
6. **Unknown is representable:** for models/routes without authoritative capability/default metadata, show `Model default` without fabricating a level. Do not send an effort field unless an explicit supported override is selected.

## Owning invariant

For every LLM dispatch, the persisted conversation choice, model/route capability metadata, UI display, telemetry, and provider wire request agree on one typed effective-effort state:

- `NativeKnown(level)`: omit the provider effort field and display the documented native level.
- `NativeUnknown`: omit the provider effort field and display an honest unknown model default.
- `Explicit(level)`: send exactly the provider-specific supported wire value and display the override.
- `Unsupported`: omit the field and offer no effort choices.

Refine the exact type design during implementation, but invalid model/effort combinations must be structurally unrepresentable or rejected before persistence. Do not use free-form strings, silent fallback, route-blind model-prefix guesses, or parallel authoritative representations.

## Scope

### Specification

- Add timeless requirements under `specs/llm/requirements.md` for capability discovery/metadata, native-default visibility, per-conversation overrides, model-switch reset, sub-agent locality, provider translation, and observability.
- Update `specs/llm/responses.allium` (or add the smallest justified companion behavioral contract) for the effort-bearing Responses request and its interaction with the existing Codex Lite reasoning context/fingerprint.
- Update `specs/llm/executive.md` with current implementation/verification status, removing stale task-relative prose encountered in touched rows per `specs/AUTHORING.md`.
- Record a project ADR if the model-native-default and explicit-override distinction needs rationale beyond the standing requirements.

### Typed capability model

- Remove GPT-5.3-Codex from `all_models`, default/fallback preference lists, model-picker fixtures, and built-in catalog tests. Do not rewrite historical conversation rows or remove generic support for an externally configured model with that wire ID.

- Extend `ModelSpec`/`ModelInfo` with route-aware, typed effort capabilities: supported levels, whether native effort is supported, and documented native default when known.
- Keep capability data model-specific and route-specific. Anthropic, direct OpenAI Responses, Codex/ChatGPT bridge, and external compatible endpoints may differ even for similar IDs.
- Give external model configuration an optional validated effort-capability declaration; absent metadata remains unknown/unsupported rather than inferred unsafely.
- Ensure the `/api/models` response lets clients render only legal choices and distinguish known native level, unknown native default, and unsupported effort.

### Conversation API and persistence

- Normalize the conversation's optional explicit effort override into a schema column, not the existing conversation JSON state. `NULL` means follow model-native behavior; a constrained typed value means explicit override.
- Thread the override through conversation creation/read APIs and model/effort update operations.
- Make model switch plus incompatible-effort reset atomic in persistence and observable as one resulting UI state.
- Preserve existing conversations as native-default (`NULL`) without pinning today's provider defaults.
- Do not inherit the parent override when creating Explore/Work sub-agent conversations.

### Provider translation

- Add a typed effort value to the common LLM request contract.
- Anthropic: translate explicit effort to `output_config: { effort: ... }` only on supporting models/routes.
- OpenAI Responses: translate explicit effort to `reasoning: { effort: ... }` while preserving other reasoning properties such as Codex Lite `context: all_turns`; one typed reasoning object must compose these non-overlapping concerns.
- Ensure the Codex WebSocket compatibility fingerprint includes the effective request field automatically, so an effort change cannot reuse an incompatible continuation.
- Keep native-default requests omitted on the wire, preserving present behavior.
- Define model-aware output-token headroom for selectable high-cost effort levels. Do not advertise `xhigh`/`max` where Phoenix's 16,384 output ceiling makes the setting misleading or likely to truncate reasoning; either raise the legal ceiling safely or exclude those choices with explicit capability metadata.

### UI

- Add effort next to model selection on new-conversation settings, showing only supported values plus `Model default`.
- Add an inline state-bar affordance while the conversation is eligible for model changes. Examples:
  - `GPT-5.6 Sol · Effort Medium (model default)`
  - `Claude Opus 4.8 · Effort Xhigh`
  - An external model with unknown semantics: `Effort Model default`
  - Haiku 4.5: no misleading effort level/selector when unsupported.
- On model switch, immediately show the compatible retained override or the atomic reset to the target's native default.
- Keep the control compact and subordinate to the primary message input.

### Usage and observability

- Add effective effort state to content-free `llm.request` telemetry and persisted turn usage so cost/quality comparisons can be grouped by model and effort without relying on today's model defaults.
- Parse OpenAI `output_tokens_details.reasoning_tokens` into a typed, non-duplicative usage representation and expose it where usage reporting can explain reasoning spend. Verify Anthropic's available usage semantics before claiming a comparable reasoning-token split.
- Do not export prompts, reasoning content, raw payloads, or reasoning summaries.

## Acceptance evidence

1. A newly created Claude Sonnet 5 conversation with `Model default` sends no effort field and displays `High (model default)`.
2. A newly created GPT-5.6 conversation with `Model default` sends no effort field and displays `Medium (model default)`.
3. A GPT-5.4 Mini native-default conversation displays `None (model default)` and sends no effort field.
4. A supported explicit override survives restart/reconnect, appears in conversation APIs/UI, request telemetry, and turn usage, and serializes to the correct provider wire shape.
5. Switching from an explicit value unsupported by the target model atomically clears the override and displays the target's native default; no request observes the new model with the stale effort.
6. Spawned sub-agents have no inherited override and resolve native behavior from their own selected model.
7. Unknown external defaults render as `Model default`, never a fabricated level.
8. GPT-5.3-Codex is absent from the built-in catalog, recommended/default/fallback choices, and model pickers; historical rows are not rewritten, and operators may still declare it as an external compatible model.
9. Unsupported models cannot represent or send an effort override.
10. Provider translation tests cover all legal levels and prove unsupported combinations cannot reach HTTP/WebSocket serialization.
11. Codex continuation tests prove changing explicit effort invalidates/rebuilds request compatibility rather than reusing an old response chain.
12. Browser tests cover creation, idle state-bar changes, model-switch reset, restart/reload, and the compact responsive layout.
13. Migration, API parity/codegen, provider unit/property tests, and `./dev.py check` pass.

## Risks and non-goals

- Native defaults are external provider behavior and may change. Capability metadata must be easy to update and clearly sourced; native requests remain omitted so Phoenix does not accidentally freeze old defaults.
- A model ID alone may not identify route semantics. Avoid assuming direct API and Codex bridge capabilities are identical.
- Removing GPT-5.3-Codex applies to Phoenix's built-in catalog only; it is not a destructive migration of persisted history and does not ban operator-configured compatible models.
- This task does not expose raw chain-of-thought or reasoning summaries.
- This task does not add a global Phoenix effort default, dynamic task classification, automatic cost optimization, or parent-to-child effort inheritance.
- This task does not claim all output tokens are reasoning tokens. Reasoning-token breakdowns must follow each provider's actual typed usage fields.
