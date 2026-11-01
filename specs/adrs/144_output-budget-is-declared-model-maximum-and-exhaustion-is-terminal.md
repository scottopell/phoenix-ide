# ADR-144: Output budget is the declared model maximum, and output exhaustion is a distinct terminal error

- **Status:** Accepted
- **Date:** 2026-10-23
- **Affects:** REQ-LLM-003, REQ-LLM-004f, REQ-LLM-006c; `LlmErrorKind`, `LlmRequest.max_tokens`, `ChatLengthFinishRejected`, `ResponsesStreamIncomplete`, `AnthropicResponseNormalized`

## Context

Phoenix chose each request's output-token budget from a Phoenix-internal
allowance keyed on reasoning effort: 16,384 tokens normally, 64,000 only when
the effective effort was `xhigh`/`max`, with a separate 4,096-token override for
continuation summaries, each then clamped to the model's configured output cap.
An external reasoning model whose native effort support is unknown never sent a
native effort field, so it fell into the small-budget branch and received only
16,384 output tokens.

For reasoning-heavy models this is a trap. A single turn can spend nearly the
whole budget on private reasoning and reach the output limit before emitting any
visible answer or completing a tool call. The provider returns a budget-limited
terminal reason (Chat `finish_reason=length`, Responses
`incomplete/max_output_tokens`, Anthropic `stop_reason=max_tokens`). Chat `length`
was classified as `invalid_response` — an auto-retryable transport-fault class —
so Phoenix retried the identical request, with the identical budget, and
exhausted it again; Responses `max_output_tokens` fell into the retryable
`server_error` default; Anthropic `max_tokens` was not treated as a failure at
all and could surface a partial tool call for execution.

Live public provider documentation shows there is no universal 16k/64k rule, that
reasoning counts against the same output allowance, and that the model's declared
maximum is the honest default headroom. It also shows the field is required on
Anthropic and optional elsewhere, with no portable "omitted means model maximum"
guarantee, and that very large maxima carry real admission, scheduling, and spend
tradeoffs on some gateways.

## Options considered

1. **Keep the effort-keyed allowance, raise the small branch.** Bump 16,384 to
   some larger constant. Cheap, but it keeps an invented Phoenix number as the
   default, keeps effort coupled to budget, and only moves the wall — a
   reasoning-heavy turn can still exhaust a fixed constant.
2. **Advertise a higher effort just to unlock the big branch.** Rejected: it
   lies about the model's capability classification and couples an unrelated
   control to budget.
3. **Default to the declared supported model maximum, clamped only by context,
   and give output exhaustion its own terminal error class.** One resolved
   budget for both the sent limit and the context reservation; no effort or
   continuation branch; exhaustion stops auto-retry and becomes user-resumable.
4. **Add a new per-request budget knob or a per-endpoint small cap now.**
   Deferred: the research shows admission/scheduling caps may eventually be
   warranted on specific gateways, but adding a knob speculatively is the kind of
   effort-split abstraction this ADR is removing.

## Decision

Adopt option 3. The request output budget is the model's **declared supported
output maximum**, clamped only by the output room the context window leaves after
the input, and the same resolved value is both sent in the provider's
output-limit field and reserved by the context planner. Budget no longer depends
on reasoning effort or on a normal-versus-continuation distinction, and Phoenix
no longer manufactures a numeric default. Built-in models carry a verified
declared maximum; an operator-configured model on a compatible HTTP backend that
declares no maximum is a configuration error rather than a silently assumed
number. Native Codex keeps its existing output-limit handling and omission.

Output-token exhaustion becomes its own `LlmErrorKind::output_tokens_exhausted`:
not auto-retryable (an unchanged request would exhaust again), user-resumable
(the user frees output room with a revised message or settings), and distinct
from `context_window_exceeded` and `invalid_response`. Chat `length`, Responses
`incomplete/max_output_tokens`, and Anthropic `stop_reason=max_tokens` all map to
it. A truncated tool call is discarded and never dispatched; already-streamed
visible text stays in the ephemeral client buffer; the truncated turn is not
persisted as a completed assistant turn.

Context fitting keeps its semantics: Phoenix trims or compacts available input to
preserve the preferred output budget rather than silently shrinking output, and
surfaces a context-policy condition when a fixed prompt plus the reserved budget
still cannot fit — never dispatching a budget that exceeds the model or context
limit.

## Consequences

- **Positive:** Reasoning-heavy turns get the model's real headroom, removing the
  artificial 16k restriction on before-first-answer generation. One resolved budget cannot drift between
  the sent limit and the reservation. Exhaustion is honest and recoverable
  instead of being retried into the same wall or masked as a transport fault.
  Unknown output limits fail loudly instead of inheriting an invented 16k.
- **Negative:** Requesting the full declared maximum can deprioritize scheduling
  or raise admission reservations on some gateways, and genuinely longer runs can
  cost more. No per-endpoint cap mitigates this yet; it is deferred until a
  concrete need appears. Operators of HTTP-backed custom models must now declare
  an output maximum.
- **Neutral:** Reuses the existing persisted `token_budget_exceeded` request
  outcome, so no outcome-schema migration is owed. Native Codex behavior and the
  malformed-response retry path are unchanged. Failed-attempt usage capture is separate work; missing usage does not imply zero billing.

## References

- Related: ADR-048 (absolute provider-attempt deadline), ADR-055 (invalid-request
  manual recovery), ADR-090 (bounded selected-model overload retry).
- Specs: `specs/llm/requirements.md` (REQ-LLM-003, REQ-LLM-004f, REQ-LLM-006c);
  `specs/llm/llm.allium` (`LlmErrorKind`, `LlmRequest`, `UserResumablePartition`);
  `specs/llm/chat-completions.allium` (`ChatLengthFinishRejected`);
  `specs/llm/responses.allium` (`ResponsesStreamIncomplete`);
  `specs/llm/anthropic.allium` (`AnthropicResponseNormalized`).
- External provider documentation on output/reasoning token accounting and
  limits: OpenAI Responses and Chat Completions API references
  (https://developers.openai.com/api/reference/resources/responses/methods/create,
  https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create)
  and reasoning guide (https://developers.openai.com/api/docs/guides/reasoning);
  Anthropic Messages (https://platform.claude.com/docs/en/api/messages/create),
  extended thinking
  (https://platform.claude.com/docs/en/build-with-claude/extended-thinking), and
  stop-reason handling
  (https://platform.claude.com/docs/en/build-with-claude/handling-stop-reasons);
  Z.ai GLM parameters (https://docs.z.ai/guides/overview/concept-param) and finish
  reasons (https://docs.z.ai/api-reference/llm/chat-completion); Databricks
  Foundation Model API limits
  (https://docs.databricks.com/aws/en/machine-learning/foundation-model-apis/limits);
  Baseten chat/reasoning reference
  (https://docs.baseten.co/reference/inference-api/chat-completions).

### Catalog evidence

Built-in Anthropic limits are decimal 128,000 for Opus 4.6/4.7/4.8/5.5 and
Sonnet 4.6/5/5.5, and 64,000 for Haiku 4.5, excluding Batch API beta limits.
References: [Anthropic models](https://platform.claude.com/docs/en/models/overview),
[Opus 4.6](https://platform.claude.com/docs/en/models/opus-4-6/overview),
[Opus 4.7](https://platform.claude.com/docs/en/models/opus-4-7/overview),
[Sonnet 4.6](https://platform.claude.com/docs/en/models/sonnet-4-6/overview), and
[Haiku 4.5](https://platform.claude.com/docs/en/models/haiku-4-5/overview).
OpenAI built-in output ceilings retain their 128,000-token catalog declarations.

Pre-dispatch fitting accounts for system instructions, tool declarations, history,
and the output reservation using the existing local estimator. This is a planning
limit, not a claim that the provider rejected the prompt. Completed responses
retain the independent 90% continuation threshold in REQ-BED-019; a future output
reservation cannot retroactively reject a successful terminal reply.
