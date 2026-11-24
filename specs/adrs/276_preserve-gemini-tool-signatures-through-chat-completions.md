# ADR-276: Preserve Gemini tool signatures through Chat Completions

- **Status:** Accepted
- **Date:** 2026-11-20
- **Affects:** REQ-LLM-002, REQ-LLM-003, REQ-LLM-004, REQ-LLM-005, REQ-LLM-007

## Context

Scott requested Gemini support through the Datadog AI Gateway, particularly
Gemini 4 Neon. The configured gateway lists Google-prefixed Gemini 4 models and
accepts the Chat Completions wire format. A bounded probe produced a tool call,
then failed its continuation with HTTP 400 when the Google thought signature
was omitted. Retaining the signature made that exchange succeed. Phoenix's
Chat Completions adapter discarded this field and had no durable replay path
for it. Provider Responses replay already had relational response-set and
ordered-item storage plus exact public-owner witnesses.

## Options considered

1. Register Gemini models without preserving signatures. This fails ordinary
   tool continuations.
2. Embed signatures in tool IDs or tool arguments. This conflates public tool
   identity or executable input with private continuation evidence.
3. Add a separate native Gemini transport and duplicate provider lifecycle
   plumbing.
4. Extend Chat Completions and the existing private output-replay infrastructure
   with typed Google tool signatures and a protocol discriminator.

## Decision

Choose option 4. Capture signed tool-call metadata in streaming and non-streaming
responses. Store each signature in an ordered child row and bind its response
set to the exact model request spelling, provider response ID, assistant-message
owner, and normalized public-content witness. Replay only onto the owning call;
reject rewritten owners before provider I/O. Preserve existing admission,
canonical-checkpoint, retry, restart, and settlement authority boundaries.
Migration 169 explicitly classifies existing output-replay rows as Responses
and admits Chat Completions in canonical receipt hashing.

Register Neon, Hydrogen, Carbon, and Argon as Google-family gateway models on
an explicitly configured Chat Completions endpoint. Their Google-prefixed IDs
match gateway discovery and requests without a private alias rewrite. External
models remain available for other routes or spellings. Ordinary function
calling does not establish tool-search or Fast support. Expose only the common
low/medium/high effort levels supported by the checked compatibility contract
and gateway; leave unverified extended mappings and native defaults unknown.

Normalize output usage from total minus prompt when a total is reported. The
live Google route reports completion and reasoning separately, while other Chat
providers include reasoning in completion; adding reasoning unconditionally
would double-count those providers.

## Consequences

- Google tools can complete across retries and process restarts without exposing
  signatures to the public transcript or changing executable arguments.
- Responses and signed Chat replay share persistence and lifecycle controls but
  remain distinct Rust variants and database protocol values.
- Google gateway entries require a configured endpoint and credentials. They do
  not accidentally call the OpenAI public API or native Codex route.
- Native Gemini-only features, direct Google authentication, and long decode are
  outside this integration. The declared output limit is the documented 65,536
  synchronous tokens within a unified 1,048,576-token context.

## References

- [Gemini 4 Neon](https://aistudio.google.com/docs/models/gemini-4-neon)
- [OpenAI compatibility](https://aistudio.google.com/docs/openai)
- [Signed tool continuation discussion](https://dd.slack.com/archives/C0BRTKLJZ6Z/p1794533627751299)
- [Gateway model and route reference](https://github.com/ddoghq-sandbox/aigw-openweights-proxy/blob/main/docs/guides/models.md)
- [LLM requirements](../llm/requirements.md)
