# LLM Provider

## User Story

As a PhoenixIDE server, I need to communicate with various LLM providers so that users can choose their preferred model while the system handles provider-specific details transparently.

## Requirements

### REQ-LLM-001: Provider Abstraction

WHEN server needs to make LLM request
THE SYSTEM SHALL use a common interface regardless of provider
AND translate to provider-specific formats internally

WHEN provider returns response
THE SYSTEM SHALL normalize to common format
AND include usage statistics when available

**Rationale:** Users benefit from model choice without the system needing provider-specific code paths in business logic.

---

### REQ-LLM-002: Backend-compatible endpoint support

WHEN an Anthropic, OpenAI Responses-compatible, or OpenAI Chat Completions-compatible endpoint is configured
THE SYSTEM SHALL use the configured URL as the exact request endpoint
AND SHALL route each model only to an endpoint matching its declared wire format
AND SHALL NOT append hidden provider-specific path suffixes

WHEN no base URL override is configured
THE SYSTEM SHALL connect directly to provider APIs

**Rationale:** Explicit endpoint URLs keep deployment configuration honest and avoid legacy gateway-root path construction.

---

### REQ-LLM-003: Model Registry

WHEN server starts with direct API access
THE SYSTEM SHALL enumerate available models based on configured API keys and base URL overrides
AND make unavailable models inaccessible

WHEN server starts with credential-helper auth and provider-compatible base URLs
THE SYSTEM SHALL query model listing endpoints derived from those base URLs when possible
AND filter configured models against discovered IDs within the matching wire-format backend
AND fall back per backend to the configured model list if model listing is unavailable or unhelpful

WHEN client requests model list
THE SYSTEM SHALL return only models that are currently available

**Rationale:** Opportunistic discovery from exact endpoint overrides lets configured models be validated without making model listing mandatory.

---

### REQ-LLM-003a: Model Discovery

WHEN deriving a model-list URL from a provider-compatible exact endpoint
THE SYSTEM SHALL replace the endpoint path with `models`
AND SHALL treat the standard `chat/completions` suffix as one endpoint path
AND SHALL skip discovery when the configured URL has no path segment to replace

WHEN a model-list endpoint returns models
THE SYSTEM SHALL match discovered IDs against configured model IDs, wire model names, and backend-prefixed aliases

WHEN model-list discovery returns no usable configured models for a wire-format backend
THE SYSTEM SHALL fall back to the configured model list for that backend
AND log warning about fallback

**Rationale:** Model listing is a validation aid, not a required deployment dependency.

---
### REQ-LLM-003b: Typed Model Effort Capability Registry

WHEN the model registry describes a model's reasoning-effort capability
THE SYSTEM SHALL classify that capability as exactly one of:
- known native support, where the provider exposes a native reasoning-effort control and Phoenix knows the provider's accepted native values
- unknown support, where Phoenix cannot prove whether a native reasoning-effort control exists or which values it accepts
- unsupported, where Phoenix knows the provider has no native reasoning-effort control

WHEN the capability is known native support
THE SYSTEM SHALL store the provider-native value mapping as part of the model specification
AND SHALL make invalid native values unrepresentable to provider translation

WHEN the capability is unknown support or unsupported
THE SYSTEM SHALL preserve the distinction between those two cases
AND SHALL NOT collapse both into a single "off" or "none" state

**Rationale:** Phoenix needs a first-class capability model so request translation, UI affordances, and conversation state can distinguish "native and known", "maybe but unknown", and "definitively absent" without parallel ad-hoc flags.

---


### REQ-LLM-004: Request Format

WHEN making LLM request
THE SYSTEM SHALL send:
- System prompt content
- Conversation message history
- Tool definitions
- Model-specific parameters

WHEN a model uses the Chat Completions backend
THE SYSTEM SHALL translate system, user, assistant, tool-call, tool-result, text, and image content to Chat Completions-compatible messages
AND SHALL preserve model-issued tool call IDs across normalization and subsequent tool-result history
AND SHALL log unsupported provider-specific content blocks before dropping them

WHEN request includes images
THE SYSTEM SHALL encode appropriately for provider
AND respect provider's image size limits

**Rationale:** Consistent request format enables the state machine to work with any provider.

---
### REQ-LLM-004a: Model-Native Reasoning Effort Omission

WHEN a request targets a model whose reasoning-effort capability is unknown support or unsupported
THE SYSTEM SHALL omit the provider-native reasoning-effort field from the translated provider request
AND SHALL NOT serialize a guessed placeholder, sentinel, or default-native value on that model's behalf

WHEN a request targets a model with known native support and the conversation has no explicit override
THE SYSTEM SHALL use the model's configured native reasoning-effort default

WHEN a request carries an explicit override that the selected model cannot represent natively
THE SYSTEM SHALL fail before provider I/O with an explicit capability mismatch rather than silently degrading to an unrelated native value

**Rationale:** A missing native field means "Phoenix is not asserting a native effort value for this model." Guessing one would create false compatibility and hide capability mismatches.

---

### REQ-LLM-004b: Per-Conversation Explicit Reasoning Effort Override

WHEN a user explicitly sets a reasoning-effort override for a conversation
THE SYSTEM SHALL persist that override as conversation state independent of the selected model's default
AND SHALL apply it to subsequent requests for that conversation until the override is reset

WHEN the user resets the override
THE SYSTEM SHALL return the conversation to model-native default behavior rather than preserving the previous explicit value implicitly

WHEN a conversation is resumed, retried, or continued through the same conversation state
THE SYSTEM SHALL preserve the explicit override exactly

**Rationale:** Reasoning effort is a conversation-level choice once the user sets it; retries and continuations must not drift back to whatever the registry default happens to be.

---

### REQ-LLM-004c: Atomic Model Switch and Override Reset

WHEN the selected model changes for a conversation
THE SYSTEM SHALL apply the new selected model and any simultaneous explicit reasoning-effort reset or replacement as one atomic state transition
AND SHALL NOT permit an intermediate persisted state where the old override is paired with the new model unintentionally

WHEN a model switch would leave the conversation with an explicit override the target model cannot represent
THE SYSTEM SHALL require the switch operation to include either a compatible replacement override or an explicit reset to model-native behavior

**Rationale:** Model selection and reasoning-effort state co-determine request translation. Updating them separately creates transient invalid combinations that can leak into retries, persistence, or sub-agent spawn snapshots.

---

### REQ-LLM-004d: Subagent Non-Inheritance of Native Reasoning Effort

WHEN Phoenix spawns a subagent conversation from a parent conversation
THE SYSTEM SHALL copy only the parent's explicit reasoning-effort override when one exists
AND SHALL NOT copy the parent's resolved model-native reasoning-effort value as if it were an explicit child setting

WHEN the parent conversation is using only model-native default behavior
THE child conversation SHALL resolve its own model-native default from its selected model independently

**Rationale:** A native default belongs to the selected model, not to the parent conversation. Copying the resolved native value into a child would freeze a provider-specific default across model boundaries and make non-overrides look user-explicit.

---

### REQ-LLM-004e: Provider Translation of Reasoning Effort

WHEN request translation targets a provider with known native reasoning-effort support
THE SYSTEM SHALL translate Phoenix's internal reasoning-effort selection to the provider's native field and native value vocabulary

WHEN request translation targets a provider without known native support
THE SYSTEM SHALL preserve the internal reasoning-effort selection in Phoenix state and observability surfaces without inventing a provider field

WHEN provider translation uses a provider-native reasoning-effort field
THE SYSTEM SHALL compose it with the rest of the provider request shape rather than replacing or bypassing the common request format

**Rationale:** Provider translation is where internal selection becomes wire shape. The common request model stays authoritative; providers only contribute native encoding.

---

### REQ-LLM-004f: Output Headroom Reservation

WHEN Phoenix prepares a request for a model with a bounded output budget
THE SYSTEM SHALL reserve output headroom separately from input-context accounting
AND SHALL apply reasoning-effort-dependent output headroom defaults before dispatch

WHEN an explicit reasoning-effort selection changes the model's expected output budget
THE SYSTEM SHALL recompute reserved output headroom from that effective selection

WHEN reporting context-window usage
THE SYSTEM SHALL distinguish consumed context from reserved output headroom rather than treating reserved output capacity as already-consumed input context

**Rationale:** Input history and reserved completion budget are different quantities. Reasoning effort can change how much output space Phoenix must reserve, so the reservation has to be explicit and recomputable.

---

### REQ-LLM-005: Response Handling

WHEN LLM responds
THE SYSTEM SHALL parse into common format containing:
- Text content blocks
- Tool use requests with IDs and parameters
- End-of-turn indicator
- Usage statistics (tokens, cost)

WHEN response indicates tool use
THE SYSTEM SHALL extract tool name, ID, and JSON input for each tool

WHEN a Chat Completions response contains private reasoning content alongside final content
THE SYSTEM SHALL omit private reasoning from user-visible normalized content
AND SHALL preserve final text and tool calls

WHEN a Chat Completions response reports cached prompt tokens
THE SYSTEM SHALL split cached tokens from uncached input tokens without double counting

**Rationale:** Normalized responses enable provider-agnostic state machine logic.

---

### REQ-LLM-006: Error Classification

WHEN LLM request fails
THE SYSTEM SHALL classify error into an explicit, named category
AND SHALL NOT use a catch-all or unknown classification

WHEN error is retryable for automatic runtime retry (network timeout, transient rate-limit throttle, server error)
THE SYSTEM SHALL include retry-after hint when available

WHEN error is not retryable for automatic runtime retry but may be recovered by user action (authentication failure, selected model overload, usage-limit window reset)
THE SYSTEM SHALL classify automatic retry policy separately from user-resume policy
AND SHALL NOT use automatic retry classification to hide a persisted conversation resume affordance

WHEN error is a quota/usage-limit exhaustion (distinct from a transient throttle)
THE SYSTEM SHALL classify it as a non-auto-retryable error category distinct from the transient rate-limit category
AND SHALL classify it as user-resumable, because the quota window resets on a clock boundary and the user can resume the conversation once it clears

WHEN error indicates the selected model is at capacity (e.g. provider returns `server_is_overloaded` or `slow_down`)
THE SYSTEM SHALL classify it as a terminal, non-retryable error category distinct from generic server errors
AND SHALL surface a message suggesting the user try a different model

WHEN a new error condition is encountered
THE SYSTEM SHALL require an explicit classification decision before it can be handled

**Rationale:** Error classification enables the state machine to implement appropriate automatic retry logic. Exhaustive classification prevents accidental behavioral contracts where unknown errors silently become non-retryable, causing transient failures to be treated as permanent. Quota exhaustion and overloaded-model errors are distinct from transient throttles — automatic retrying them is wasted work and the user-facing recovery differs (wait for window reset / upgrade plan / pick another model). Automatic retry safety is not the same capability as user-triggered resume after external action; auth failures are non-auto-retryable but resumable after credentials are refreshed.

---

### REQ-LLM-006a: Plan-Aware Quota Messages (Codex Backend)

WHEN a request through the codex backend (`chatgpt.com/backend-api/codex`) fails with HTTP 429 and a body indicating quota exhaustion
THE SYSTEM SHALL parse the structured error payload to extract:
- the user's plan type (e.g. plus, pro, team, business, enterprise, free)
- the quota reset timestamp when present
- the primary and secondary rate-limit window snapshots from response headers (used percent, window minutes, reset-at)
- the credits snapshot (has-credits, unlimited, balance) when present
- the active limit identifier and limit name when present
- the optional promotional message from the response headers

WHEN rendering a quota-exhaustion error to the user
THE SYSTEM SHALL produce a plan-aware message that names the recovery action appropriate to the user's plan (upgrade path for consumer plans, admin-contact for workspace plans, credit purchase for paid plans with depleted credits)
AND SHALL include the absolute reset time formatted in the user's local timezone when known

WHEN a 429 response from the codex backend does NOT indicate quota exhaustion (i.e. it is a transient per-window throttle)
THE SYSTEM SHALL classify it as the retryable transient rate-limit category, not as quota exhaustion

WHEN a request through any backend that is NOT the codex backend returns 429
THE SYSTEM SHALL apply the provider's existing generic error path
AND SHALL NOT attempt to parse codex-specific structured fields

**Rationale:** Phoenix's codex bridge routes ChatGPT-plan-backed traffic to a backend that returns structured quota state in both the response body (plan type, reset timestamp) and headers (window snapshots, credits, promo messages). Surfacing this structure as opaque text strands the user — they cannot tell whether to wait, upgrade, or contact an admin. The codex CLI (the canonical client for the same backend) already renders these strings; adopting the same wording avoids divergence with what users see in adjacent tools.

---

### REQ-LLM-007: Usage Tracking

WHEN LLM response includes token counts
THE SYSTEM SHALL record input tokens, output tokens, and cache statistics

WHEN tracking context window usage
THE SYSTEM SHALL compute total as input + output + cache tokens

**Rationale:** Users need visibility into token consumption for context window management.

---
### REQ-LLM-007a: Reasoning Token and Effort Observability

WHEN Phoenix records usage or request telemetry for a response
THE SYSTEM SHALL record the effective reasoning-effort selection used for that request
AND SHALL record provider-reported reasoning-token usage when the provider exposes it

WHEN the provider does not expose reasoning-token usage
THE SYSTEM SHALL preserve the distinction between "not reported" and zero rather than fabricating a zero-valued reasoning-token count

WHEN a response is surfaced to users or operators through observability interfaces
THE SYSTEM SHALL make reasoning-effort and reasoning-token facts available without requiring access to raw provider payloads

**Rationale:** Reasoning effort is a user-visible control only if Phoenix can later explain what effective setting ran and, when available, how much provider-reported reasoning budget it consumed.

---

### REQ-LLM-008: Request Observability

WHEN an LLM request executes
THE SYSTEM SHALL emit bounded structured telemetry containing model, provider, transport, duration, token counts, retry attempt, a request identifier, conversation identifiers when conversation context exists, and classified failure reason when applicable
AND SHALL keep human-readable log filtering independent from exported trace filtering

WHEN the system measures provider time to first token (TTFT)
THE SYSTEM SHALL measure from the request transmission or dispatch boundary on the selected transport to the first provider event that carries generated model output
AND SHALL count generated text deltas, reasoning deltas, tool-call deltas, and other generated structured-output deltas as generation-bearing events
AND SHALL NOT count acknowledgements, pings, keepalives, metadata-only events, usage-only events, or terminal events as TTFT-bearing events

WHEN the system records or aggregates TTFT
THE SYSTEM SHALL preserve provider, model, and transport as first-class dimensions
AND SHALL support aggregate analysis by those dimensions

WHEN traces are exported
THE SYSTEM SHALL export only explicitly designated Phoenix spans
AND SHALL NOT export tracing events, dependency diagnostics, prompts, conversation content, tool schemas or arguments, authorization credentials or headers, raw provider payloads, WebSocket frames, SSE deltas, or parser buffers
AND SHALL enforce finite span attribute, event, and link limits before export

WHEN provider streaming diagnostics are logged locally
THE SYSTEM SHALL log only content-free structural metadata such as event type, byte count, parser counters, transport transition, classified error code, and timing milestones

WHEN request observability is persisted, exported, or surfaced for aggregate analysis
THE SYSTEM SHALL keep TTFT telemetry content-free and privacy-preserving
AND SHALL NOT include prompts, generated content, tool arguments, or any raw provider event payload needed only to reconstruct user-visible content

**Rationale:** Operators need request latency, TTFT, usage, correlation, retry, failure, and transport visibility without turning long-lived LLM spans into unbounded payload stores or exposing user content and credentials. Provider-centric TTFT must reflect provider generation onset rather than client-visible wording so that queueing, scheduling, prefill, and transport behavior remain comparable across providers and transports.

---
### REQ-LLM-008a: Reasoning Effort in Request Observability

WHEN an LLM request executes
THE SYSTEM SHALL include the effective reasoning-effort selection, the model capability classification that justified its wire encoding or omission, and any reserved output headroom in bounded structured telemetry

WHEN the selected provider request omits a native reasoning-effort field because support is unknown or unsupported
THE SYSTEM SHALL make that omission observable as an intentional classified outcome rather than leaving it implicit in missing provider payload fields

**Rationale:** Operators need to tell the difference between "effort omitted because unsupported", "effort omitted because unknown", and "effort sent natively" without inspecting raw request bodies.

---

### REQ-LLM-009: Streaming Responses

WHEN LLM provider supports streaming
THE SYSTEM SHALL deliver partial text content as it arrives from the provider
AND accumulate tool input fragments internally until complete
AND assemble a final structured response identical to the non-streaming path

WHEN provider does not support streaming
THE SYSTEM SHALL fall back to the non-streaming request path

WHEN streaming connection is interrupted mid-response
THE SYSTEM SHALL treat it as a retryable network error

WHEN a Chat Completions streaming request is sent
THE SYSTEM SHALL request a terminal usage chunk
AND SHALL accumulate text, tool-call fragments, finish reason, and usage into the final normalized response

WHEN a Chat Completions stream returns an inline error payload
THE SYSTEM SHALL classify and surface that error
AND SHALL NOT normalize it as an empty successful response

WHEN a Chat Completions stream ends without a terminal finish reason or completion sentinel
THE SYSTEM SHALL reject the incomplete stream as an invalid response

**Rationale:** Token-by-token streaming enables progressive display of LLM output (REQ-BED-025). The provider layer must deliver partial content while still producing the same final response type for the state machine.

---

### REQ-LLM-010: Native Codex Authentication

WHEN the Codex bridge authenticates with ChatGPT
THE SYSTEM SHALL use only the credential created by Phoenix's native Codex login flow
AND SHALL NOT read or modify Codex CLI authentication state

WHEN the user signs out of Codex in Phoenix
THE SYSTEM SHALL invalidate the same credential that owns Codex model access and quota status

**Rationale:** A single credential owner keeps model availability, quota identity, account switching, and sign-out behavior consistent.
