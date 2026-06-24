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

WHEN an Anthropic or OpenAI-compatible base URL is configured
THE SYSTEM SHALL use the configured URL as the exact request endpoint
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
AND filter configured models against discovered IDs
AND fall back to the configured model list if model listing is unavailable or unhelpful

WHEN client requests model list
THE SYSTEM SHALL return only models that are currently available

**Rationale:** Opportunistic discovery from exact endpoint overrides lets configured models be validated without making model listing mandatory.

---

### REQ-LLM-003a: Model Discovery

WHEN deriving a model-list URL from a provider-compatible base URL
THE SYSTEM SHALL replace the final path segment with `models`
AND SHALL skip discovery when the configured URL has no path segment to replace

WHEN a model-list endpoint returns models
THE SYSTEM SHALL match discovered IDs against configured model IDs, wire model names, and backend-prefixed aliases

WHEN model-list discovery returns no usable configured models
THE SYSTEM SHALL fall back to the configured model list
AND log warning about fallback

**Rationale:** Model listing is a validation aid, not a required deployment dependency.

---

### REQ-LLM-004: Request Format

WHEN making LLM request
THE SYSTEM SHALL send:
- System prompt content
- Conversation message history
- Tool definitions
- Model-specific parameters

WHEN request includes images
THE SYSTEM SHALL encode appropriately for provider
AND respect provider's image size limits

**Rationale:** Consistent request format enables the state machine to work with any provider.

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

### REQ-LLM-008: Request Logging

WHEN LLM request completes
THE SYSTEM SHALL log model, duration, token counts, and any errors

**Rationale:** Operational visibility into LLM requests for monitoring and troubleshooting.

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

**Rationale:** Token-by-token streaming enables progressive display of LLM output (REQ-BED-025). The provider layer must deliver partial content while still producing the same final response type for the state machine.
