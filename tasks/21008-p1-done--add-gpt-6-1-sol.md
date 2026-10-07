# Add GPT-6.1 Sol support

Register GPT-6.1 Sol with its direct API effort, context and output capabilities. Verify Codex sign-in eligibility and request shape rather than inheriting GPT-6 Sol transport assumptions. Keep pricing truthful where long-context per-request facts are absent. Document new GPT-6 features separately; async tool calling and mid-turn steering are not in this implementation.

## Verified model contract

- [OpenAI model page](https://developers.openai.com/api/docs/models/gpt-6.1-sol): `gpt-6.1-sol`, 1,050,000-token direct context, 128,000-token output, low/medium/high/xhigh/max efforts, native API default medium. No `none` or `minimal`; use Responses for tool calls.
- [OpenAI pricing](https://developers.openai.com/api/docs/pricing): published Standard and Fast rates vary by whether a request crosses 272K input; an aggregate Phoenix turn lacks request-level route and threshold evidence, so historical cost remains unknown.
- [Upstream Codex catalog](https://github.com/openai/codex/blob/main/codex-rs/models-manager/models.json): exact slug, Responses Lite, preferred WebSockets, 272K default context (872K optional maximum). Its `low` preset is a client choice; it does not establish the API-native default. Phoenix keeps the existing Codex 272K cap and does not enable 872K here.
- Provider catalog discovery is advisory under REQ-LLM-004h. A model can appear in Phoenix but be rejected for an account that lacks rollout/entitlement; Phoenix surfaces that error without switching accounts or billing routes.

## New GPT-6 platform features outside this model-registration change

- [Async tool calling](https://developers.openai.com/api/docs/guides/async-tool-calling): `async: true` on function/custom tools lets reasoning continue while the application holds an unfinished tool call and returns its result under the original `call_id`. Phoenix does not yet model pending tool results across concurrent response continuations.
- [Mid-turn steering](https://developers.openai.com/api/docs/guides/steering): `response.steer` queues new instructions on a running Responses WebSocket. Phoenix's existing steering queue is not this provider protocol; no new wire event or running-response update is enabled.
- [Reasoning configuration updates](https://developers.openai.com/api/docs/guides/reasoning#change-reasoning-mid-conversation): a `configuration_update` input item can change effort without rewriting the cached prefix in supported single-agent flows. Phoenix still uses request-level effort.
- [Misalignment monitoring](https://developers.openai.com/api/docs/guides/safety-checks/misalignment-monitoring): asynchronous safeguards announced for GPT-6 Astra; no Sol-specific integration is claimed.
- Existing GPT-6 capabilities include computer use, structured outputs, streaming, programmatic tools, multi-agent orchestration, prompt caching, persisted reasoning, compaction and Pro mode; model/endpoint compatibility varies. This task does not enable those features where Phoenix lacks an existing implementation.
