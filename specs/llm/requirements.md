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

### REQ-LLM-002: Gateway Support

WHEN exe.dev gateway URL is configured
THE SYSTEM SHALL route all LLM requests through the gateway
AND append provider-specific path suffixes

WHEN gateway is not configured
THE SYSTEM SHALL connect directly to provider APIs

**Rationale:** exe.dev environment provides a gateway that handles API keys and routing, simplifying deployment.

---

### REQ-LLM-003: Model Registry

WHEN server starts with direct API access
THE SYSTEM SHALL enumerate available models based on configured API keys
AND make unavailable models inaccessible

WHEN server starts with gateway configured
THE SYSTEM SHALL query gateway model listing endpoints at startup
AND merge discovered models with hardcoded metadata
AND fall back to hardcoded list if gateway doesn't support model listing

WHEN client requests model list
THE SYSTEM SHALL return only models that are currently available

**Rationale:** Dynamic discovery from gateway enables new models to appear automatically without code changes. Hardcoded fallback ensures Phoenix works with minimal gateways or direct API mode.

---

### REQ-LLM-003a: Model Discovery

WHEN querying gateway for model list
THE SYSTEM SHALL request from provider-specific endpoints:
- `{gateway}/anthropic/v1/models` (requires `anthropic-version` header)
- `{gateway}/openai/v1/models`
- `{gateway}/fireworks/inference/v1/models`

WHEN gateway returns models
THE SYSTEM SHALL extract model IDs and metadata (context_length, capabilities)
AND merge with hardcoded descriptions and defaults

WHEN gateway endpoint returns 404 or error
THE SYSTEM SHALL fall back to hardcoded model list
AND log warning about fallback

**Rationale:** Provider-specific endpoints return different metadata formats. Graceful degradation ensures Phoenix works regardless of gateway capabilities.

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

WHEN error is retryable (network timeout, rate limit, server error)
THE SYSTEM SHALL include retry-after hint when available

WHEN a new error condition is encountered
THE SYSTEM SHALL require an explicit classification decision before it can be handled

**Rationale:** Error classification enables the state machine to implement appropriate retry logic. Exhaustive classification prevents accidental behavioral contracts where unknown errors silently become non-retryable, causing transient failures to be treated as permanent.

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
