# LLM provider contract refactor

## Summary

Refactor Phoenix's LLM provider boundary so gateway/provider compatibility is explicit, typed, and regression-tested instead of discovered piecemeal in PR review.

This task is the follow-up vision from the Chat Completions gateway hardening PR. It is intentionally medium-sized, but all five slices below are required for completion because they reinforce the same invariant: Phoenix must not treat "OpenAI-compatible" as a single uniform contract.

Do **not** preserve `OPENAI_BASE_URL` as a compatibility shim in the final design. Endpoint configuration should be explicit by protocol/API format.

## Motivating use cases

- **Mixed API-format model switching:** A user should be able to switch a conversation between a Responses-style model and a Chat Completions-style model without Phoenix accidentally sending one protocol's request body to the other protocol's endpoint.
- **Gateway-specific optional fields:** Operators should be able to add an OpenAI-compatible gateway model that rejects optional OpenAI-native fields without every normal turn failing on unsupported request parameters.
- **Long-running streaming turns:** If a gateway drops a stream, filters content, hits the output limit, or omits usage chunks, Phoenix should preserve the correct terminal/error semantics instead of persisting partial output as success or recording misleading zero-usage turns.
- **Small-context external models:** Adding a small-context model should not require operators to know continuation internals; default output caps and thresholds should stay safe without immediate continuation loops.
- **Provider additions by future contributors:** Adding a new provider-compatible model should force the contributor to declare endpoint protocol, capabilities, error behavior, and streaming fixtures up front, rather than relying on post-hoc PR review to discover missing cases.

## Required slices

1. **Capability flags for Chat Completions models**
   - Add explicit per-model/provider capability modeling for optional Chat Completions fields.
   - At minimum cover:
     - `parallel_tool_calls`
     - `stream_options.include_usage`
   - External `openai_chat_completions` models should default conservatively.
   - Built-in/provider-native models may opt into richer capabilities.
   - Request serialization must read capability fields rather than guessing from backend alone.

2. **Endpoint config cleanup**
   - Remove ambiguous runtime use of `OPENAI_BASE_URL`.
   - Use explicit protocol-specific endpoint config only, such as:
     - `OPENAI_RESPONSES_BASE_URL`
     - `OPENAI_CHAT_COMPLETIONS_BASE_URL`
   - Startup/registry configuration should make it structurally impossible to route a Responses request to a Chat Completions endpoint or vice versa.

3. **Protocol-specific error classifiers**
   - Split shared OpenAI-ish error classification into protocol/backend-specific classifiers.
   - Separate at least:
     - OpenAI Responses
     - Codex/ChatGPT backend Responses behavior
     - OpenAI-compatible Chat Completions
   - Codex-specific terminal codes must not leak into generic Chat Completions gateway behavior unless explicitly modeled.

4. **Typed provider termination boundary**
   - Introduce a provider-level termination/result enum closer to runtime truth than `Result<LlmResponse, LlmError>`.
   - It should represent semantic outcomes directly, such as:
     - completed response
     - rate limited
     - usage/quota exhausted
     - output limit exceeded
     - content filtered
     - context window exceeded
     - auth failed
     - invalid request
     - invalid response
     - server overloaded
     - server/network failure
   - Runtime/state-machine conversion should become a total, mostly mechanical mapping from typed termination values.

5. **Fixture-based provider compatibility matrix**
   - Add reusable fixtures/tests for provider and gateway contract variants.
   - Cover request-shape expectations and stream/error terminal behavior.
   - Include cases for:
     - OpenAI-native behavior
     - external Chat Completions gateway with conservative optional-field support
     - gateway rejecting optional fields
     - numeric and string error codes
     - `finish_reason: length`
     - `finish_reason: content_filter`
     - refusal-only responses
     - unterminated streams
   - Future provider additions should fail CI unless their capabilities and behavior are represented in the matrix.

## Acceptance criteria

- [ ] Optional Chat Completions request fields are capability-gated, not emitted based only on backend/tool presence.
- [ ] Ambiguous `OPENAI_BASE_URL` runtime routing is removed; protocol-specific endpoint config is required.
- [ ] Error classifiers are protocol/backend-scoped and tested independently.
- [ ] Provider completion/termination results are represented as typed semantic outcomes before runtime/state-machine mapping.
- [ ] A compatibility fixture matrix covers the gateway/provider edge cases that repeatedly surfaced in review.
- [ ] Documentation and `specs/llm` describe the new configuration and contract model.
- [ ] Existing Chat Completions gateway QA scenarios continue to work.
