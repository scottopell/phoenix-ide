# exe-dev AI Gateway — Input Contract

> What clients (shelley, phoenix-ide) send to an exe-dev gateway and what they
> expect back. Implementation details (credential injection, upstream routing)
> are out of scope. Primary source: shelley. Secondary: phoenix-ide.

## 1. Discovery

Shelley reads `llm_gateway` from a JSON config file passed via the global
`-config` flag (before the subcommand):

```
shelley -config config.json serve --port 8033
```

```json
{ "llm_gateway": "http://localhost:8462" }
```

Phoenix-IDE reads the same field from `/exe.dev/shelley.json`, defaulting to
`http://169.254.169.254/gateway/llm`.

> Shelley does **not** read a `LLM_GATEWAY` env var.

When a gateway is configured, all provider API keys default to `"implicit"`.

## 2. Routes

All routes are `{gateway_base} + suffix`. Trailing slashes on the base are
stripped.

### Shelley (canonical)

| Provider  | Suffix                                      | Type     |
|-----------|---------------------------------------------|----------|
| Anthropic | `/_/gateway/anthropic/v1/messages`          | Terminal |
| OpenAI    | `/_/gateway/openai/v1`                      | Base     |
| Fireworks | `/_/gateway/fireworks/inference/v1`         | Base     |
| Gemini    | `/_/gateway/gemini/v1/models/generate`      | Terminal |

"Base" routes have sub-paths appended by the client SDK (`/chat/completions`,
`/responses`). Gemini is defined but not currently gateway-enabled.

### Phoenix-IDE (alternate)

Same routes without the `/_/gateway` prefix: `/anthropic/v1/messages`,
`/openai/v1/chat/completions`, etc. A gateway SHOULD support both forms.

## 3. Provider contracts

Each provider uses its **native API format**. The gateway never translates.

### 3.1 Anthropic — `/_/gateway/anthropic/v1/messages`

Headers: `X-API-Key: implicit`, `Anthropic-Version: 2023-06-01`

Body/response: native Anthropic Messages API. Streaming via `"stream": true`
returns `text/event-stream` with Anthropic SSE events.

### 3.2 OpenAI — `/_/gateway/openai/v1/chat/completions`

Headers: `Authorization: Bearer implicit`

Body/response: native OpenAI Chat Completions API.

### 3.3 OpenAI Responses — `/_/gateway/openai/v1/responses`

Same headers as 3.2. Body/response: native OpenAI Responses API (codex models).

### 3.4 Fireworks — `/_/gateway/fireworks/inference/v1/chat/completions`

Headers: `Authorization: Bearer implicit`

Body/response: OpenAI-compatible Chat Completions. Model names use Fireworks
identifiers (`accounts/fireworks/models/...`).

## 4. Client headers

Forwarded transparently by the gateway:

| Header                    | Value                          | When            |
|---------------------------|--------------------------------|-----------------|
| `User-Agent`              | `Shelley/{commit_8chars}`      | Always          |
| `Shelley-Conversation-Id` | UUID                           | When available  |
| `x-session-affinity`      | UUID                           | Fireworks only  |

## 5. Streaming

`"stream": true` in the request body means the response is SSE
(`text/event-stream`). Events must be forwarded without buffering in the
provider's native SSE format.

## 6. Errors

The gateway returns upstream errors faithfully. Clients retry on 429 and 5xx
with their own backoff. Gateway-level failures use 502/503.

## 7. Models

Clients do **not** query the gateway for models. Model lists are hardcoded.
No `/models` endpoint is needed.

## 8. AI Gateway implementation notes

Empirical findings when backing the exe-dev gateway with Datadog AI Gateway
(`ai-gateway.us1.staging.dog`):

**`provider` header** — AI Gateway defaults all requests to OpenAI. Native
Anthropic requests to `/v1/messages` fail without a `provider: anthropic`
header. The gateway must inject this based on the route; clients don't send it.

**Model prefixing** — The OpenAI-compat endpoint (`/v1/chat/completions`)
routes by model prefix (`openai/gpt-4.1`, `anthropic/claude-...`). Bare names
default to OpenAI. Native endpoints with the `provider` header don't need
prefixes.

**Supported providers** — `openai`, `anthropic`, `gemini`, `google`,
`bedrock-anthropic`, `datadoginternal`, `mock`. **Fireworks is not supported.**

**Auth** — `Authorization: Bearer {ddtool_jwt}` plus `source` and `org-id`
headers. The client's `X-API-Key: implicit` is harmless to forward.
