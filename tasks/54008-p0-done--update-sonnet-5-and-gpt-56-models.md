# Update Phoenix built-in models for Sonnet 5 and GPT-5.6

## Context

The built-in Phoenix model registry previously defaulted to `claude-sonnet-4-6` and included OpenAI models through `gpt-5.5`. Sonnet 5 has shipped and should replace Sonnet 4.6 as the default. GPT-5.6 is available through the AI Gateway Responses endpoint as explicit `gpt-5.6-sol`, `gpt-5.6-luna`, and `gpt-5.6-terra` variants; the hyphenated `gpt-5-6` spelling is not available.

## Scope

Update Phoenix's built-in LLM model list and default-selection behavior.

Primary code surfaces identified:

- `crates/phoenix-llm/src/models.rs` — built-in `ModelSpec` entries in `all_models()`
- `crates/phoenix-llm/src/registry.rs` — `pick_default_model()` preference order, hardcoded fallback, helper/test registries, mid-tier/cheap model preferences if applicable
- `crates/phoenix-ide/src/api/usage.rs` — pricing table, if public pricing is known
- Tests and fixtures containing `claude-sonnet-4-6` expectations where those expectations represent the registry default rather than legacy data

## Plan

1. Verify exact Sonnet 5 public model ID and API wire name.
   - Expected shape, based on current stable IDs: `claude-sonnet-5`, but confirm before editing.
2. Add a Sonnet 5 `ModelSpec` with the correct `id`, `api_name`, backend, description, context window, `recommended: true`, and tool-search support.
3. Demote `claude-sonnet-4-6` to legacy/recommended false once Sonnet 5 is present.
4. Update default model selection to prefer Sonnet 5 before Sonnet 4.6 and update the no-service fallback from Sonnet 4.6 to Sonnet 5.
5. Re-check GPT-5.6 availability.
   - Live AI Gateway probing confirmed `gpt-5.6-sol`, `gpt-5.6-luna`, and `gpt-5.6-terra` work and `gpt-5-6` does not.
   - Add explicit GPT-5.6 variant entries as built-in OpenAI Responses models, make them recommended, place them before `gpt-5.5`, and add pricing if known.
6. Update tests/fixtures whose expected default or helper model should move to Sonnet 5. Preserve historical/migration tests that intentionally reference old model IDs.
7. Run focused tests, then `./dev.py check`.

## Acceptance Criteria

- [x] Phoenix's registry default is Sonnet 5 when Anthropic auth is available and no `DEFAULT_MODEL` override is set.
- [x] `claude-sonnet-4-6` remains usable for existing conversations but is no longer the preferred default/recommended Sonnet.
- [x] `/api/models` reports Sonnet 5 as available when Anthropic auth/credential-helper routing is available.
- [x] GPT-5.6 is added only if a live availability check confirms the model ID; otherwise the final implementation notes that GPT-5.6 was not yet accessible.
- [x] Usage pricing is updated for new models when public pricing is known; otherwise unknown pricing is explicit rather than guessed.
- [x] Tests and generated expectations are updated without rewriting migration/history tests that intentionally cover legacy IDs.
- [x] `./dev.py check` passes.

## Notes

The existing LLM spec requirements are model-list agnostic; this is primarily a built-in registry data/default update rather than a requirements change.
