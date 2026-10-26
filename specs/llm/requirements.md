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

WHEN selecting a product default without an available explicit `DEFAULT_MODEL` override
THE SYSTEM SHALL prefer exact `gpt-6.1-sol` when registered
AND SHALL retain available provider fallback choices when that model is unavailable
AND SHALL NOT replace explicit conversation, task, or specialized worker choices

THE built-in OpenAI Responses catalog SHALL contain `gpt-5.6-luna`, `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-6-astra`, `gpt-6.1-sol`, `gpt-6-luna`, and `gpt-6-sol`
AND SHALL NOT contain `gpt-5.4-mini`, `gpt-5.4`, or `gpt-5.5`
AND provider discovery SHALL NOT introduce a model that is absent from both the built-in and operator-configured catalogs

WHEN an exact operator-configured route uses an identifier that is also a legacy built-in identifier
THE SYSTEM SHALL preserve the exact operator-configured route rather than replace its identifier

WHEN resolving a persisted live model pin or named-worker pin whose exact route is unavailable
THE SYSTEM SHALL map `gpt-5.3-codex`, `gpt-5.4`, and `gpt-5.5` to `gpt-5.6-sol`
AND SHALL map `gpt-5.4-mini` to `gpt-5.6-luna`
AND SHALL require the mapped replacement to be available instead of falling back to the deployment default

THE SYSTEM SHALL preserve the original model identity on historical turns and requests or runtimes whose model was already resolved
AND SHALL NOT use catalog family or version ordering to infer orchestration qualification for an unlisted model

WHEN a model specification describes its output capability
THE SYSTEM SHALL treat the model's declared supported output maximum as the authoritative source of its output budget (REQ-LLM-004f)
AND SHALL require that maximum to be a verified catalog fact for every built-in model
AND SHALL reject an operator-configured model on a compatible HTTP backend that carries no declared output maximum with a clear configuration error rather than substituting an invented numeric default

**Rationale:** Opportunistic discovery from exact endpoint overrides validates configured models without making model listing mandatory. Explicit compatibility mappings keep live pins deterministic while leaving historical attribution, in-flight work, and operator-defined routes authoritative. The declared output maximum is what sets each request's budget, so an unknown maximum must fail honestly instead of silently manufacturing one.

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

WHEN a user requests a conversation model change
THE SYSTEM SHALL admit it only from Idle, Error, or RecoverableContinuationFailure
AND SHALL evaluate admission against the authoritative effective conversation state
AND SHALL reject the change in every other state without modifying configuration or queuing a deferred switch.

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

### REQ-LLM-004g: Fast Request Speed

WHEN a conversation uses a model and provider route that advertise Fast request speed
THE SYSTEM SHALL allow the user to select Standard or Fast independently of model reasoning effort
AND SHALL disclose that Fast increases request speed and usage cost

WHEN Fast is selected
THE SYSTEM SHALL translate it to the supported route's native request-speed control
AND SHALL send every required feature header for that control
AND SHALL NOT silently retry the request at Standard speed after a provider failure

WHEN Standard is selected
THE SYSTEM SHALL omit provider-native Fast controls from the request

WHEN the selected model or provider route does not support Fast mode
THE SYSTEM SHALL NOT advertise Fast capability
AND SHALL reject an explicit attempt to enable Fast
AND SHALL reset an inherited conversation selection to Standard when switching to that route

WHEN Phoenix records usage for a turn
THE SYSTEM SHALL persist the effective Standard or Fast selection with that turn
AND SHALL use that immutable value for speed-dependent historical cost calculations

WHEN a successful provider response reports effective request speed
THE SYSTEM SHALL parse it as a closed provider-native value
AND SHALL reject a missing, unknown, or contradictory Fast response before usage persistence
AND SHALL retain requested effective speed as the sole historical pricing authority

WHEN Phoenix spawns a subagent conversation
THE child conversation SHALL start with Standard request speed independently of the parent's selection

WHEN a conversation continues into a successor conversation
THE successor SHALL preserve the parent's request-speed selection

**Rationale:** Fast is a paid request-routing choice, not a model or reasoning-effort alias. Provider routes encode it differently. Route-aware capability gating prevents Phoenix from claiming support where the native control is unavailable, while per-turn persistence keeps historical pricing independent from mutable conversation settings.

---

### REQ-LLM-004h: Codex Model Discovery and Route Identity

WHEN ChatGPT/Codex authentication has a configured connection and loaded credential with an identified account
THE SYSTEM SHALL advertise and route Phoenix-supported built-in Codex models independently of whether provider discovery lists their exact wire identifiers

WHEN Codex model discovery fails or omits a Phoenix-supported built-in model
THE SYSTEM SHALL treat that result as advisory
AND SHALL surface an honest provider model error if execution rejects the selected model
AND SHALL NOT substitute another model, account, authentication route, or billing route

WHEN provider discovery lists a model absent from both the built-in and operator-configured catalogs
THE SYSTEM SHALL NOT register or qualify that model

WHEN direct OpenAI API authentication is configured
THE SYSTEM SHALL determine direct model availability independently of the ChatGPT account catalog

WHEN a model supports different native reasoning-effort values across direct OpenAI and Codex account routes
THE SYSTEM SHALL publish and validate the reasoning-effort capability of the selected route
AND SHALL NOT send direct-only values through the Codex route

WHEN the active Codex credential changes while model discovery is in flight
THE SYSTEM SHALL NOT publish the discovered catalog with a different account's credential

**Rationale:** Phoenix's built-in catalog defines the exact models and protocols Phoenix supports, while provider listing can omit executable models and therefore cannot deny a supported route. Credential and account binding still protect route identity, and provider execution remains authoritative for actual account entitlement without fallback to another identity or billing path.

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

### REQ-LLM-004f: Output Budget Resolution and Reservation

WHEN Phoenix prepares an ordinary or continuation agent-turn request for a model with a bounded output budget
THE SYSTEM SHALL resolve one output-token budget equal to the model's declared supported output maximum
AND SHALL recover input room through the context-management policy when that preferred budget does not fit
AND SHALL NOT silently reduce that budget to accommodate retained input
AND SHALL use that single resolved budget both as the value sent in the provider's output-limit field and as the output headroom the context planner reserves
AND SHALL NOT derive the budget from reasoning effort, a continuation-versus-normal distinction, or a Phoenix-internal fixed allowance

WHEN reporting context-window usage
THE SYSTEM SHALL distinguish consumed input context from reserved output budget rather than treating reserved output capacity as already-consumed input context

WHEN the input plus the preferred reserved output budget does not fit the context window
THE SYSTEM SHALL first trim or compact available input to recover room for the preferred output budget rather than silently shrinking the request to a tiny output budget
AND SHALL, when a fixed prompt plus the reserved output budget still cannot fit, surface the condition through the context-management policy rather than dispatching a request whose output budget exceeds the model or context limit

Bounded utility requests such as titles, command suggestions, search interpretation, and structured QA MAY declare a smaller output limit for their specific contract. They SHALL use that same explicit limit for their own request accounting and SHALL NOT change the ordinary or continuation agent-turn default.

WHEN fitting an ordinary request before dispatch
THE SYSTEM SHALL account for its system prompt, finalized retained tool declarations and restrictions, rendered history including nested images, matching provider-private replay, and output reservation
AND SHALL attempt permitted stale-tool-result clearing before reporting a planned-capacity failure
AND SHALL distinguish conservative local planning from authoritative provider token counts

The completed-response continuation threshold remains governed by `specs/bedrock/requirements.md` REQ-BED-019; reserved future output SHALL NOT lower that threshold or reject an otherwise acceptable completed response.

**Rationale:** The model's capability ceiling is the right default request budget; a smaller Phoenix-internal allowance tied to reasoning effort starved reasoning-heavy turns and produced output-exhaustion failures before any visible answer. One resolved value keeps the sent limit and the reserved headroom from diverging, while context fitting protects input rather than quietly collapsing the output budget.

---

### REQ-LLM-004i: Tool Availability and Provider Continuation

WHEN tool execution eligibility changes during a conversation
THE SYSTEM SHALL preserve historical calls, arguments, results, errors, and required provider references
AND SHALL retain the latest authentic declaration for each tool name independently of current execution eligibility
AND SHALL check current execution authority before dispatching every new invocation
AND SHALL require admission under the originating request's callable policy and dispatch only to a tool whose input schema equals its authentic originating declaration, allowing live changes only to revoke that admission
AND SHALL return one matching unavailable error result without execution when authority is absent

WHEN preparing a provider request
THE SYSTEM SHALL render the current conversation policy using only controls supported by the selected route and model
AND SHALL bind request preparation and dispatch to the same resolved provider service so a registry reload cannot retarget prepared private continuation
AND SHALL preserve required provider-private continuation through retry and restart
AND SHALL scope Codex continuation to its selected account identity, preserving it through same-account token refresh and retiring it before requests under another account
AND SHALL admit Codex routes only when their account identity is known
AND SHALL retain complete Responses tool-round output envelopes, including returned assistant phase, until exchange settlement
AND SHALL keep provider-specific controls and private data outside portable public messages

WHEN a provider switch settles or authoritatively abandons an exchange
THE SYSTEM SHALL atomically retire its native continuation context with the selected-model settings
AND SHALL preserve conversation tool policy and public history
AND SHALL establish current policy afresh when returning to a provider
AND SHALL NOT revive retired private data, native events, or transport continuation handles

WHEN explicit context management removes a native tool-change anchor
THE SYSTEM SHALL establish a fresh native tool prefix and current policy at a legal visible position
AND SHALL NOT relocate individual historical native events or rewrite active private replay owners

WHEN a historical tool reference lacks an authentic retained declaration
THE SYSTEM SHALL report the missing declaration and preserve the exchange for retry
AND SHALL NOT fabricate a schema from historical arguments or assume that uncaptured declarations can be recovered

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

WHEN a provider returns signed or encrypted private blocks that are required to continue an unsettled tool exchange
THE SYSTEM SHALL store those blocks in provider-private replay state outside public transcript content
AND SHALL bind every private response set to its durable public assistant-message identity
AND SHALL replay each block unchanged at its validated original ordinal
AND SHALL retain all required private response sets until the exchange settles or is authoritatively abandoned
AND SHALL reject missing, rewritten, malformed, unknown, unsigned, or incorrectly positioned replay state before provider I/O

WHEN the accepted response starts or extends private replay
THE SYSTEM SHALL persist replay mutation with the active execution-state transition before tool execution
AND SHALL apply replay mutation only after stale-generation and reducer admission checks

WHEN the exchange settles or is authoritatively abandoned
THE SYSTEM SHALL clear private replay with the terminal message, state, settings, or handoff transaction that establishes settlement
AND SHALL NOT recreate a cleared response identity from a stale outcome

WHEN Phoenix serializes public messages, reconnect state, transcript APIs, search data, exports, errors, diagnostics, or logs
THE SYSTEM SHALL NOT include readable thinking, signatures, encrypted redacted data, or malformed raw provider payloads

WHEN Phoenix rebuilds a supported Anthropic request prefix that no longer matches preserved-thinking binding
THE SYSTEM SHALL request provider-side block dropping instead of freezing Phoenix's prompt prefix
AND SHALL record only bounded content-free transformation diagnostics

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

WHEN error is retryable under the generic automatic runtime policy (network timeout, transient rate-limit throttle, server error)
THE SYSTEM SHALL include retry-after hint when available
AND SHALL use 3 total attempts with nominal waits of 2 seconds and 4 seconds

WHEN error is retryable under the selected-model overload policy
THE SYSTEM SHALL classify it separately from the generic automatic runtime policy

WHEN error is not retryable for automatic runtime retry but may be recovered by user action (authentication failure, usage-limit window reset)
THE SYSTEM SHALL classify automatic retry policy separately from user-resume policy
AND SHALL NOT use automatic retry classification to hide a persisted conversation resume affordance

WHEN error is a quota/usage-limit exhaustion (distinct from a transient throttle)
THE SYSTEM SHALL classify it as a non-auto-retryable error category distinct from the transient rate-limit category
AND SHALL classify it as user-resumable, because the quota window resets on a clock boundary and the user can resume the conversation once it clears

WHEN error indicates the selected model is at capacity (e.g. provider returns `server_is_overloaded`, `slow_down`, or `overloaded_error`)
THE SYSTEM SHALL classify it as `ServerOverloaded`, distinct from quota exhaustion, authentication failure, prompt rejection, invalid request, and generic server error
AND SHALL apply the dedicated selected-model overload retry policy in REQ-LLM-006b

WHEN the provider rejects the assembled prompt under its prompt policy (for example Responses API code `invalid_prompt`)
THE SYSTEM SHALL classify it as a prompt-rejection category distinct from a malformed request
AND SHALL NOT automatically replay the identical rejected request
AND SHALL classify the persisted error as user-resumable so a revised message or explicit continuation can recover within the same conversation

WHEN a provider returns a successful transport response whose terminal reason is refusal
THE SYSTEM SHALL discard any partial generated content from that response
AND SHALL classify the outcome as prompt rejection
AND SHALL NOT treat it as a tool-use continuation or an automatically retryable server failure

WHEN a streaming attempt fails, is refused, retries, or is cancelled after visible token deltas
THE SYSTEM SHALL clear only that attempt's ephemeral client buffer
AND SHALL NOT let a stale error or retry event clear a newer request's buffer

WHEN a request fails with an invalid-request classification
THE SYSTEM SHALL stop automatic retries of that request
AND SHALL allow the user to retry or dismiss the error within the same conversation
AND SHALL preserve the conversation identity and persisted transcript through that recovery
AND SHALL apply this user-resume policy to persisted invalid-request errors without requiring an error-text match or a server restart

WHEN a new error condition is encountered
THE SYSTEM SHALL require an explicit classification decision before it can be handled

**Rationale:** Error classification enables the state machine to select the appropriate automatic retry policy. Exhaustive classification prevents unknown errors from silently inheriting retry behavior. Quota exhaustion, model capacity, authentication failure, prompt rejection, and malformed requests require distinct recovery behavior. Automatic retry safety is also separate from user-triggered resume after external action or revised input.

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

### REQ-LLM-006b: Bounded Selected-Model Overload Retry

WHEN an ordinary turn or continuation-summary request first receives `ServerOverloaded`
THE SYSTEM SHALL retain the selected model and retry that same logical request against that same model
AND SHALL NOT change models, purchase capacity, or open a second retry loop
AND SHALL set an absolute overload deadline 120 seconds after the first overload observation

WHEN scheduling selected-model overload retries
THE SYSTEM SHALL permit at most 5 total provider attempts
AND SHALL use nominal waits of 4, 8, 16, and 32 seconds before attempts 2 through 5
AND SHALL apply deterministic jitter in the inclusive range of minus 25 percent through plus 25 percent
AND SHALL derive that jitter from the stable logical request identity and target attempt so restart computes the same wait

WHEN a valid provider `Retry-After` hint is present and does not exceed 30 seconds
THE SYSTEM SHALL use the hint as a floor for the jittered wait

WHEN a valid provider `Retry-After` hint exceeds 30 seconds
THE SYSTEM SHALL stop automatic retry visibly
AND SHALL NOT truncate the hint to 30 seconds

WHEN a computed retry time reaches or exceeds the absolute overload deadline
THE SYSTEM SHALL stop automatic retry visibly
AND SHALL NOT renew the deadline

WHEN overload retry is waiting or in flight
THE SYSTEM SHALL persist the logical target, target attempt, retry time when waiting, first-overload time, and absolute deadline as conversation state

WHEN the runtime restarts with persisted overload retry state before its deadline
THE SYSTEM SHALL rearm a future retry once, dispatch a due retry once, or redispatch an in-flight target once according to the persisted phase
AND SHALL preserve the original target attempt and deadline

WHEN the runtime restarts at or after the persisted deadline
THE SYSTEM SHALL expire the overload retry without provider dispatch

WHEN selected-model overload retry stops because of attempt exhaustion, an over-limit provider hint, or deadline exhaustion
THE SYSTEM SHALL enter ordinary terminal `Error` for an ordinary turn
AND SHALL enter `RecoverableContinuationFailure` for continuation-summary generation while retaining the continuation operation identity and inputs

WHEN a user cancels an overload retry or Close settles the active conversation
THE SYSTEM SHALL retire both retry-timer authority and provider-admission authority
AND SHALL reject stale timer and provider outcomes

WHEN automatic continuation opens a successor request
THE SYSTEM SHALL use the request's single overload loop
AND SHALL NOT create another overload loop at the continuation-opening boundary

**Rationale:** Selected-model capacity is transient enough to merit bounded same-model retry, but it must not silently change the user's routing choice or outlive its accepted logical operation. Stable jitter prevents synchronized retries without making restart renew the wait. Persisted phase and absolute time bounds make process recovery deterministic, while target-specific terminal states preserve continuation identity.

---

### REQ-LLM-006c: Output-Token Exhaustion Classification

WHEN a provider response terminates because the model reached the request's output-token budget (Chat Completions `finish_reason` length, Responses `incomplete` with reason `max_output_tokens`, or Anthropic `stop_reason` max_tokens)
THE SYSTEM SHALL classify the outcome as a dedicated output-token-exhaustion category distinct from context-window exhaustion and from the malformed-response category
AND SHALL NOT automatically retry the identical request, because repeating unchanged parameters does not correct the budget limit
AND SHALL classify the persisted error as user-resumable so a revised message or revised settings that free output room can recover within the same conversation

WHEN an output-token-exhausted response carries a partial or truncated tool call
THE SYSTEM SHALL discard that tool call and SHALL NOT dispatch it

WHEN visible content was already streamed to the user before the output budget was reached
THE SYSTEM SHALL preserve that already-visible content under the existing streaming-buffer policy
AND SHALL NOT persist the truncated turn as a completed assistant turn

WHEN the input plus reserved output cannot fit the context window
THE SYSTEM SHALL classify that condition as context-window exhaustion rather than output-token exhaustion

The native Codex transport keeps its existing output-limit field handling and official terminal reasons unchanged; a provider response whose shape is malformed rather than budget-limited remains in the malformed-response category with its existing retry behavior.

This classification reuses the existing persisted request-outcome value for token-budget exhaustion; it introduces no new persisted outcome value and therefore requires no outcome-schema migration. Missing failed-attempt usage SHALL NOT be interpreted as zero billing; extending failed-attempt usage capture is outside this contract.

**Rationale:** An output budget stop is a well-formed provider response, not a transport fault, so retrying the identical request only wastes attempts and private reasoning. Users recover by freeing output room, so the error is terminal for automatic retry but resumable by the user. Keeping it separate from context-window exhaustion preserves distinct recovery guidance, and never dispatching a truncated tool call prevents a partial, structurally invalid call from executing.

---

### REQ-LLM-007: Usage Tracking

WHEN LLM response includes token counts
THE SYSTEM SHALL record input tokens, output tokens, and cache statistics

WHEN tracking context window usage
THE SYSTEM SHALL compute total as input + output + cache tokens

WHEN a model's token price depends on per-request context thresholds or route details that historical usage does not preserve
THE SYSTEM SHALL report its historical cost as unknown
AND SHALL NOT estimate cost from an incomplete aggregate pricing model

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

#### REQ-LLM-009a: Responses Terminal Classification

WHEN a Responses transport receives an authenticated `response.completed` terminal event whose complete nonempty output consists only of structurally completed assistant messages containing no public content and structurally valid, non-incomplete reasoning items
AND every reasoning item, when present, has a positive reasoning-token subset of the billed output tokens
AND no reasoning tokens are reported when reasoning items are absent
AND the stream did not previously expose non-empty visible output or a function call in stream events
THEN Phoenix SHALL settle the request as a valid quiet end turn without fabricating visible assistant text.

WHEN any stream event exposes non-empty visible output or a function call
THEN Phoenix SHALL retain that evidence through terminal assembly and SHALL NOT settle quietly if a later event overwrites or omits the output item.

WHEN reasoning-token usage is present but is not a nonnegative in-range integer
THEN Phoenix SHALL classify the result as a retryable provider failure rather than treating the malformed value as absent or zero.

WHEN a Responses terminal includes reasoning items but does not report a positive reasoning-token subset of billed output
THEN Phoenix SHALL classify the result as a retryable provider failure rather than treating it as an unbilled empty turn.

WHEN billed output has no terminal model-visible content and the terminal shape does not prove a structurally valid quiet terminal
THEN Phoenix SHALL classify the result as a retryable provider failure rather than silently persisting an empty agent turn.

WHEN an HTTP/SSE Responses stream ends before a terminal event
THEN Phoenix SHALL classify the attempt as an interrupted retryable network failure and SHALL NOT fabricate completed status from partial output.

The HTTP/SSE and WebSocket transports SHALL apply the same terminal normalization to equivalent authenticated terminal events.

**Rationale:** Reasoning is authentic private provider output, not public transcript text. A completed reasoning-only turn is legitimate when usage and terminal shape prove it, while missing terminal content and pre-terminal EOF remain evidence of transport/provider loss.

---

### REQ-LLM-010: Native Codex Authentication

WHEN the Codex bridge authenticates with ChatGPT
THE SYSTEM SHALL use only the credential created by Phoenix's native Codex login flow
AND SHALL NOT read or modify Codex CLI authentication state

WHEN the user signs out of Codex in Phoenix
THE SYSTEM SHALL invalidate the same credential that owns Codex model access and quota status

**Rationale:** A single credential owner keeps model availability, quota identity, account switching, and sign-out behavior consistent.

---

### REQ-LLM-011: Provider Attempts Have an Absolute Total Deadline

WHEN Phoenix dispatches an LLM provider attempt
THE SYSTEM SHALL capture one absolute total deadline that bounds credential resolution, connection establishment, transport sends and reads, provider activity, transport recovery, fallback, body consumption, and response normalization for that attempt
AND SHALL NOT renew the deadline because of provider events, non-visible generation, keepalives, reconnects, authentication retry, or fallback

WHEN a local connect, frame, or request timeout applies
THE SYSTEM MAY fail that operation earlier
BUT SHALL NOT treat a sequence of local timeouts as a substitute for the total deadline

WHEN the total deadline elapses before a complete response wins
THE SYSTEM SHALL cancel the process-local provider work
AND SHALL classify the attempt as `timed_out` distinctly from network failure and user cancellation
AND SHALL preserve content-free partial stream telemetry with `completed = false`
AND SHALL route the typed outcome through the current-generation retry or durable terminal-settlement path

WHEN a timed-out attempt is retried
THE SYSTEM SHALL retain the accepted durable turn and canonical user-message identity
AND SHALL allocate a distinct provider request identity
AND SHALL prevent a late result from an expired generation from persisting response content, executing tools, or altering terminal state

WHEN the finite retry budget is exhausted
THE SYSTEM SHALL durably persist the timeout failure and release conversation ownership through the direct-turn atomic terminal-settlement boundary

### REQ-LLM-012: LLM Attempt Outcomes Are Total and Idempotent

WHEN an LLM attempt reaches Phoenix's typed result boundary
THE SYSTEM SHALL finalize exactly one request-metric outcome as success, a provider failure class, `timed_out`, or `cancelled`

WHEN timeout, cancellation, success, or a late result races with another terminal classification
THE SYSTEM SHALL preserve the first terminal classification
AND SHALL make repeated persistence of the same request-and-retry identity idempotent

### REQ-LLM-013: Timeout Outcome Schema Migration Preserves Existing Metrics

WHEN a database without the `timed_out` LLM request outcome is upgraded
THE SYSTEM SHALL migrate the outcome constraint transactionally
AND SHALL preserve every existing metric row and outcome value unchanged
AND SHALL admit `timed_out`
AND SHALL continue rejecting undeclared outcome values

The migration is forward-only internal persistence under the project compatibility policy. It does not establish downgrade compatibility or a stable cross-version SQLite schema contract.

---

### REQ-LLM-014: Canonical Model Identity Is Distinct From Request Addressing

THE SYSTEM SHALL treat a model's Phoenix ID as the sole canonical identity for the picker, persistence, continuation, agent/worker selection, and every capability or transport check

THE SYSTEM SHALL represent the wire spelling used to address a provider request as a separate typed value, scoped to serialization, discovery-name matching, and replay/continuation binding only

AND SHALL NOT accept that typed request-addressing value anywhere a feature, capability, or transport check reads canonical model identity

AND SHALL NOT introduce a third identity field that duplicates either the canonical ID or the request-addressing value

WHEN a configured model declares a wire model name
THE SYSTEM SHALL preserve that configured name as the model's default request spelling
AND SHALL NOT mutate the model's canonical spec to apply a deployment-specific request-addressing override

WHEN resolving the request spelling for an outbound call
THE SYSTEM SHALL first resolve the model's actual backend route
AND SHALL then resolve the request spelling for that route
AND SHALL ignore a configured request-addressing override when the resolved route is the native Codex bridge

WHEN an operator configures `PHOENIX_LLM_REQUEST_MODELS` as an inline JSON map of backend route to Phoenix model ID to request spelling
THE SYSTEM SHALL accept only the three backend-route keys corresponding to Anthropic, OpenAI Responses, and OpenAI Chat Completions
AND SHALL reject the Codex bridge as a route key
AND SHALL reject an unknown route, an unknown model ID, a model ID whose backend does not match its route key, a blank request spelling, duplicate route or model keys, and malformed JSON
AND SHALL validate the entire map atomically before any entry takes effect, so a single invalid entry discards the whole configured map rather than applying a partial one

WHEN a request-addressing override changes which spelling is sent on the wire
THE SYSTEM SHALL change the derived continuation/route key accordingly
AND SHALL NOT change the model's canonical ID, capability classification, or the continuation/route key produced in the absence of an override

THE SYSTEM SHALL NOT automatically inject a `provider` request header inferred from model or route identity
AND SHALL continue to honor explicitly configured custom headers and the headers required for native Codex account binding and protocol framing

WHEN normalizing an OpenAI Responses API reply, whether delivered as a single JSON body or reassembled from a stream
THE SYSTEM SHALL bind the normalized response's model field to the request spelling that was sent
AND SHALL NOT bind it to the physical model name the provider reports in its reply, which may legitimately differ from the request spelling under a gateway or alias

WHEN matching a model against provider-reported model-list discovery
THE SYSTEM SHALL match using the model's resolved request spelling for its actual route, plus its canonical ID and normative backend-prefixed legacy aliases
AND SHALL continue to treat the Codex advisory catalog as non-binding: absence from it SHALL NOT deregister a supported built-in, and presence in it SHALL NOT register a model absent from the built-in or operator-configured catalog

**Rationale:** Conflating canonical identity with wire spelling forced every capability and transport check to special-case gateway-rewritten model names, and made an automatically inferred `provider` header silently wrong whenever a deployment's request spelling didn't match its actual provider. Splitting the two lets the picker, persistence, and capability checks stay keyed on a value that never changes under a gateway, while the wire spelling can be remapped per deployment through one validated, atomic configuration surface that native Codex — which owns its own account-bound addressing — is structurally excluded from.

### REQ-LLM-004j: One Capacity Decision for the Prepared Request

WHEN preparing an ordinary or tool-free continuation request
THE SYSTEM SHALL resolve effective context capacity from the selected model and route
AND SHALL freeze the complete input, tool declarations and restrictions, applicable private replay, generation settings, output reservation, and project-guidance snapshot before the capacity decision
AND SHALL dispatch that same prepared request or discard its plan if any frozen input changes
AND SHALL keep live execution authority and revocation checks authoritative at dispatch.

THE SYSTEM SHALL distinguish ready, reducible input pressure, and irreducible capacity conflict as exhaustive planning outcomes
AND SHALL distinguish locally planned pressure from an authoritative provider context rejection
AND SHALL count cached input as occupying context.

WHEN preferred output plus input does not fit
THE SYSTEM SHALL attempt permitted worthwhile stale-result clearing, then route an eligible parent to durable compaction if retained history can be reduced
AND SHALL expose a recoverable capacity conflict when mandatory input and preferred output cannot fit
AND SHALL NOT retry the unchanged impossible plan or create a successor with an error notice as its handoff.

Ordinary and continuation requests SHALL share capacity accounting, but MAY assemble different input under their retention contracts. Compaction SHALL NOT recursively compact itself. A sub-agent SHALL use its existing bounded terminal failure contract rather than create a parent continuation.

### REQ-LLM-004k: Counting Evidence and Bounded Fallback

WHEN determining input occupancy
THE SYSTEM SHALL distinguish provider-counted, verified local-tokenizer, and conservatively estimated counts
AND SHALL bind a count to the exact prepared request and selected route
AND SHALL NOT treat model aliases, a shared API protocol, or public text tokenization alone as proof of complete request accounting.

WHEN local planning would otherwise clear, compact, or reject input and the configured route explicitly supports input counting
THE SYSTEM SHALL attempt at most one remote count across all candidates in one logical request-planning operation, including its clearing and handoff planning, with a finite absolute timeout and no counting retry
AND SHALL use a supported count as planning evidence with any provider-documented uncertainty
AND SHALL fall back to local planning on unavailable, failed, or timed-out counting without changing model, route, or credentials.

THE SYSTEM SHALL compare predicted occupancy with actual provider usage only for the same dispatched prepared request
AND SHALL preserve unknown usage as unknown
AND SHALL NOT require remote counting for requests local planning accepts, automatic tokenizer downloads, endpoint capability probing, or a durable counting workflow.

### REQ-LLM-004l: Model and Route Changes Preserve Work

WHEN a selected model or route changes
THE SYSTEM SHALL admit the change only under REQ-LLM-004c and apply the accepted selection to subsequent requests
AND SHALL NOT change an in-flight request or introduce a queued model-switch lifecycle
AND SHALL replan against the new effective limits and output maximum
AND SHALL preserve portable public history, tool-call/result pairing, user corrections, and unresolved work
AND SHALL retain provider-private replay and transport continuation state only when the destination route's existing compatibility contract permits it
AND SHALL NOT silently select another model to avoid a capacity conflict.

A switch SHALL use the same clearing, compaction, and conflict policy as other input pressure. A supported switch SHALL NOT require cache reuse or a shared tokenizer between routes.

### REQ-LLM-004m: Predictable Prefix Changes

WHEN input sources, retained history, model, route, and relevant settings are unchanged
THE SYSTEM SHALL produce identical provider-visible historical input and declaration ordering
AND SHALL append new activity without re-rendering old observations into new mutable summaries.

THE SYSTEM SHALL refresh ordinary repository guidance and skill metadata at a new user-turn boundary and retain that snapshot throughout the turn's tool loop
AND SHALL keep authoritative permission changes live regardless of snapshot stability.

WHEN clearing, compaction, instruction refresh, model/route changes, or tool-policy changes require a prefix change
THE SYSTEM SHALL preserve correct semantics even when cache reuse is lost
AND SHALL identify the deliberate change in request diagnostics
AND SHALL NOT promise provider cache residency, hit rate, or cross-model reuse.

For Codex-auth, direct API, and compatible gateway routes, cache controls SHALL be selected from that route's verified capabilities rather than inferred from protocol name alone.
