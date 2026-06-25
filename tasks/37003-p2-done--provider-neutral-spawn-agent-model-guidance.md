# Make spawn_agents model guidance provider-neutral

## Problem

The `spawn_agents` tool schema currently nudges assistants toward Anthropic-only model aliases:

- `mode` description says Explore uses a “haiku model”.
- `model` description gives examples like `claude-haiku-4-5` and `claude-sonnet-4-6`.

In environments where those aliases are unavailable, assistants sometimes follow the guidance anyway, trigger an “unknown model” error, and then have to retry on the available model.

## Plan

1. Update `crates/phoenix-tools/src/subagent.rs` tool schema copy to be provider-neutral:
   - Explore mode: describe it as using the registry/provider-selected cheap model.
   - Work mode: describe it as inheriting the parent model.
   - `model` override: remove hard-coded Anthropic examples and say it must be one of the environment’s available model IDs.
2. Preserve existing behavior in `runtime/executor.rs`:
   - Explicit model override still must exist in `ModelRegistry`.
   - Explore defaults still use `cheap_model_id_for_provider(&parent_model)`.
   - Work defaults still inherit the parent model.
3. Add or update a focused schema test so the spawn_agents schema no longer contains hard-coded Anthropic example aliases in the user-facing guidance.
4. Run the relevant Rust test lane for `phoenix-tools` (or the targeted test if faster), then `./dev.py check` if practical.

## Acceptance criteria

- The rendered `spawn_agents` tool guidance no longer mentions `claude-haiku-4-5`, `claude-sonnet-4-6`, or “haiku model”.
- The guidance accurately describes registry-driven defaults and explicit override validation.
- Existing model resolution behavior remains unchanged.
