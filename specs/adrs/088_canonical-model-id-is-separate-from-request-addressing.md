# ADR-088: Canonical model ID is separate from request addressing

- **Status:** Accepted
- **Date:** 2026-10-09
- **Affects:** `crates/phoenix-llm/src/models.rs`, `anthropic.rs`, `openai.rs`, `registry.rs`, `service.rs` (`specs/llm/`)

## Context

`ModelSpec` carried one field, `api_name`, for both the model's provider wire name and its implicit identity for feature gating. Capability checks (`is_fast_mode`, `supports_native_tool_changes`, `supports_responses_lite`, `effort_capabilities_for`, codex catalog membership) matched on `spec.api_name`. `LlmServiceImpl::headers_for_provider` separately derived a `provider` header value from the same field by splitting on `/` and auto-injecting it whenever any header or base-url override was present.

This conflation broke under gateway/alias deployments: a model with a rewritten `api_name` (e.g. `gateway/anthropic/claude-sonnet-5-5`) silently lost its capability classification because the check string no longer matched, and the auto-injected `provider` header guessed wrong whenever the configured gateway prefix wasn't the actual upstream provider. There was no supported way to remap only the wire spelling for one deployment without also changing the value every capability check keyed on.

## Options considered

1. **Keep one field, special-case gateway prefixes in every capability check.** Rejected — pushes gateway awareness into every call site that reads `api_name`, with no structural guarantee a new check gets it right.
2. **Add a second optional "display id" field and keep `api_name` as the single source for both wire spelling and capability matching, falling back to it when display id is unset.** Rejected — two overlapping fields for the same deployment create exactly the "wrong states representable" problem the project's correctness principles rule out; call sites still have to know which one to read.
3. **Make the model's Phoenix `id` the sole canonical identity for every capability/transport/persistence check, and introduce a distinct typed `RequestModelName` scoped to serialization, discovery matching, and replay/continuation binding.** Chosen.

## Decision

`ModelSpec.id` is canonical. All feature, capability, and transport checks (`is_fast_mode`, `supports_native_tool_changes`, `supports_responses_lite`/`supports_explicit_prompt_cache`, `effort_capabilities_for`, `service_tier_capabilities_for`, Codex catalog advisory logging) read `spec.id`, never a request-addressing value.

`RequestModelName` is a newtype wrapping the wire spelling. `ModelSpec.default_request_name` replaces `api_name`; configured `api_name` in `PHOENIX_LLM_MODELS` continues to set it, unchanged, as the default. `LlmServiceImpl` holds a separate `request_name` field (defaulting to `spec.default_request_name`, overridable via `with_request_name`) so a deployment-specific remap never mutates the shared `ModelSpec` the registry hands out. Every provider translation layer (Anthropic request `model`, Responses request/replay/bind, Chat Completions request/response) takes `&RequestModelName` as an explicit parameter alongside `&ModelSpec`; no function accepts a `RequestModelName` where it previously read a capability off `api_name`.

`PHOENIX_LLM_REQUEST_MODELS` is an inline JSON map keyed by the three HTTP backend routes (`anthropic`, `openai_responses`, `openai_chat_completions`) to a Phoenix model ID to request spelling. It is parsed and validated once at startup; any unknown route, unknown model ID, route/backend mismatch, blank spelling, duplicate keys, or an entry naming the Codex bridge route fails the whole map atomically rather than applying a partial one. Route resolution happens first: a built-in OpenAI Responses model routed through the native Codex bridge always uses its own `default_request_name` and ignores the map, because Codex owns its own account-bound addressing.

The automatic `provider` header inference in `headers_for_provider` is deleted outright rather than reconciled against the new type split — it inferred from the same conflated identity this ADR separates, and the project's explicit-over-implicit header policy (REQ-LLM-014) treats inference as the defect, not a feature to preserve under a new name. Explicitly configured custom headers and the headers required for native Codex account binding/protocol framing are unaffected.

Responses replay and normalization (`bind_responses_model`, `finalize_responses_stream`) bind the response's `model` field to the request spelling that was sent, not the physical name the provider may report back under a gateway alias. Discovery matching (`spec_matches_discovered_model`) matches against the resolved request name for the model's actual route, plus the canonical ID and backend-prefixed legacy aliases; it does not match bare `api_name` as a separate string now that `default_request_name` is request-only.

## Consequences

- **Positive:** A gateway-remapped request spelling can no longer desync capability gating — every check reads `id`, which a deployment alias never touches.
- **Positive:** `PHOENIX_LLM_REQUEST_MODELS` gives one validated, atomic surface for deployment-specific wire remaps instead of requiring a fork of the built-in catalog.
- **Positive:** Removing automatic header inference removes a silent-wrong-guess failure mode; the explicit-custom-header and native-Codex-header paths are simpler to audit.
- **Negative:** Deployments that relied on the auto-injected `provider` header must now configure it explicitly via `LLM_CUSTOM_HEADERS` if their gateway needs it.
- **Neutral:** The continuation/route key changes when request spelling is overridden; request limits remain determined by canonical model capability, by design (REQ-LLM-014); no database migration is needed because the key's shape and default values are unchanged when no override is configured.

## References

- `specs/llm/requirements.md` REQ-LLM-014
- `specs/llm/executive.md`
- Key symbols: `phoenix_llm::models::RequestModelName`, `ModelSpec::default_request_name`, `registry::RequestModelOverrides`, `LlmServiceImpl::with_request_name`
