# GPT-6 Astra provider contract

This implementation was compared against OpenAI Codex commit
[`654b0a77d0d2f81aa21f61caf7af4be88fe550bb`](https://github.com/openai/codex/commit/654b0a77d0d2f81aa21f61caf7af4be88fe550bb)
and the official [GPT-6 Astra API model page](https://developers.openai.com/api/docs/models/gpt-6-astra).

The upstream Codex authorities inspected were:

- `codex-rs/models-manager/models.json` for catalog metadata;
- `codex-rs/core/src/client.rs` and `client_tests.rs` for Responses Lite and WebSocket behavior;
- `codex-rs/codex-api/src/endpoint/models.rs` for the account catalog endpoint;
- `codex-rs/protocol/src/models.rs` and `codex-rs/core/src/codex.rs` for the special Codex `ultra` orchestration mode.

## Evidence matrix

| Capability | Direct OpenAI API | ChatGPT/Codex |
|---|---|---|
| Phoenix/model wire ID | `gpt-6-astra` | `gpt-6-astra` |
| Availability authority | OpenAI Responses API/model docs | Active account response from `/backend-api/codex/models` |
| Context window | 1,050,000 tokens | 272,000 tokens; upstream has an experimental 872,000-token feature mode Phoenix does not expose |
| Max output | 128,000 tokens | Not separately declared in the Codex catalog; Phoenix's Codex context cap remains authoritative |
| Reasoning effort | `low`, `medium`, `high`, `xhigh`, `max`; default `low` | Same provider-facing values |
| Responses request | Platform Responses shape | Responses Lite with compatibility metadata/header |
| Preferred transport | HTTP streaming | WebSocket with safe HTTP/full-request fallback |
| Prompt caching | Explicit platform cache controls | Automatic cache key; no direct-platform cache options |
| Service tier | Standard/Fast (`priority`) | Standard/Fast (`priority`) |
| Modalities | Text and image input; text output | Phoenix's existing text/image Responses translation |
| Base token prices per 1M | $10 input, $12.50 cache write, $1 cache read, $50 output | ChatGPT plan quota, not direct token billing |
| Recommended/default | Officially “most capable” | Highest upstream catalog priority and default model |

## Deliberate exclusions

Upstream Codex advertises an `ultra` UI effort for Astra, but it is not an
additional provider wire value. Codex remaps `ultra` to `xhigh`, enables a
multi-agent orchestration mode, and changes agent limits. Phoenix does not
implement that orchestration contract, and the direct API documents efforts
only through `max`, so Phoenix does not expose `ultra` as a `ModelEffort`.

The upstream experimental 872K Codex context mode is also not represented as a
second Phoenix model or a hidden context override. Phoenix uses the ordinary
272K Codex bridge cap and the documented 1.05M direct-platform window.

The direct API pricing page applies higher rates above 272K input tokens and
multipliers for Batch, Flex, and Fast processing. Codex-auth turns consume a
ChatGPT quota rather than direct token billing. Phoenix's usage rows do not
retain the auth route and service tier needed to distinguish those cases, so
Astra pricing remains explicitly unknown rather than reporting a misleading
base-rate estimate.
