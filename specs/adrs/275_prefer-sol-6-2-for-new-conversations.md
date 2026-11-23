# ADR-275: Prefer Sol 6.2 for new conversations

- **Status:** Accepted
- **Date:** 2026-11-20
- **Affects:** REQ-LLM-003

## Context

Scott selected GPT-6.2 Sol as Phoenix's product default after comparing its
published Standard rates with GPT-6.1 Sol. Both list $2 input, $0.10 cached
input, and $10 output per million tokens. Existing conversation and worker pins
are independent choices.

## Options considered

1. Retain GPT-6.1 Sol as the product default.
2. Prefer GPT-6.2 Sol for new conversations while preserving explicit defaults
   and existing pins.

## Decision

Choose option 2. Put exact GPT-6.2 Sol first in the product preference list and
retain GPT-6.1 Sol and the provider fallbacks when it is unavailable. Available
explicit DEFAULT_MODEL values continue to win. Preserve existing model and
effort overrides. Leave the native effort default unknown where the checked
provider documentation does not establish it; omit an unset override.

## Consequences

- New conversations without an explicit model choose GPT-6.2 Sol when available.
- This source change does not rewrite existing conversations or operator pins.
- API Flex remains a distinct option on GPT-6.1 Sol; GPT-6.2 Sol has no Flex tier.

## References

- [OpenAI API pricing](https://developers.openai.com/api/docs/pricing)
- [LLM requirements](../llm/requirements.md)
