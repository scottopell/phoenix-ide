# Add configurable additive LLM model specs

## Context

Phoenix currently discovers models from gateways/base URLs, but only registers discovered model IDs that match the built-in `ModelSpec` list. That blocks low-friction trials of new provider/gateway models such as the Baseten open-weight POC: the endpoint may already be Anthropic Messages-compatible, but Phoenix still needs code changes just to make the model IDs selectable.

We want an additive configuration mechanism so operators can register extra model specs via environment configuration without recompiling Phoenix. Built-in models remain the default source of truth; configured models only add to the registry unless they intentionally match an existing ID under a well-defined policy.

## Goal

Allow Phoenix to register additional LLM models from env-var configuration, suitable for open-weight/provider POCs exposed through existing supported wire protocols.

## Proposed scope

1. Add a typed external model spec format loaded from inline environment configuration.
   - Use inline JSON via `PHOENIX_LLM_MODELS` as the scoped mechanism for now.
   - Do not add a file-path based loader in this task; avoiding disk dependency keeps deployment/config refresh behavior simple.
   - Shape should include at least:
     - `id` (Phoenix model ID and default provider wire model name)
     - optional `api_name` only when the provider requires a different wire model name than `id`
     - `provider` or provider family compatible with current routing
     - `api_format` (`anthropic` initially; possibly `openai_responses` if useful)
     - `description`
     - `context_window`
     - `recommended`
     - `supports_tool_search`
2. Merge configured specs additively with `all_models()` before registration/discovery filtering.
   - Built-in models continue to work unchanged.
   - Unknown configured models can be registered when the selected auth/base-url mode supports their provider family.
   - Discovery should consider configured models as known models, not silently ignore them.
3. Define duplicate-ID behavior explicitly.
   - Recommended: reject duplicate IDs from external config by default, log a clear error/warning, and keep the built-in model.
   - Avoid silent overrides unless a separate explicit override feature is requested later.
4. Ensure config errors are diagnosable.
   - Invalid JSON, unsupported `api_format`, invalid context windows, or duplicate IDs should produce actionable startup logs.
   - No secrets should be logged.
5. Document the workflow in README/env docs with a Baseten-style example.

## Baseten POC example target

An operator should be able to add a model file like:

```json
[
  {
    "id": "baseten/moonshotai/Kimi-K2.6",
    "provider": "anthropic",
    "api_format": "anthropic",
    "description": "Baseten Kimi K2.6 open-weight POC",
    "context_window": 262000,
    "recommended": false,
    "supports_tool_search": false
  }
]
```

and configure Phoenix with:

```env
LLM_API_KEY_HELPER=ddtool auth token rapid-ai-platform --datacenter us1.staging.dog
LLM_AUTH_HEADER=bearer
ANTHROPIC_BASE_URL=https://ai-gateway.us1.staging.dog/v1/messages
LLM_CUSTOM_HEADERS=source: openweight-restricted-poc-<firstname>-<lastname>\norg-id: 2\nx-target-account: eval
DEFAULT_MODEL=baseten/moonshotai/Kimi-K2.6
PHOENIX_LLM_MODELS='[{"id":"baseten/moonshotai/Kimi-K2.6","provider":"anthropic","api_format":"anthropic","description":"Baseten Kimi K2.6 open-weight POC","context_window":262000,"recommended":false,"supports_tool_search":false}]'
```

## Acceptance criteria

- Phoenix can register at least one externally configured Anthropic-compatible model without code changes, with `api_name` omitted and defaulting to `id`.
- The configured model appears in `/api/models` and can be selected as `DEFAULT_MODEL` when credentials/base URL are configured.
- Gateway/model discovery includes configured models in the allowlist instead of ignoring them as unknown.
- Built-in model behavior is unchanged when no external model config is present.
- Invalid inline external model config fails safe with clear logs and does not remove built-in models.
- README documents the env vars and includes an Anthropic-compatible provider POC example.

## Non-goals

- Implementing OpenAI Chat Completions support.
- General provider plugin architecture.
- Runtime hot-reload of model config without restart.
- Overriding built-in model definitions.
