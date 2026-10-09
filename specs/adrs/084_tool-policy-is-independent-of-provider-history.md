# ADR-084: Tool policy is independent of provider history

- **Status:** Accepted
- **Date:** 2026-10-07
- **Affects:** REQ-LLM-004i, REQ-LLM-005; ToolHistoryIntegrity, ProviderRequestProjection

## Context

Refreshing an MCP catalog after an execution error can remove a declaration. Phase changes also withdraw tools. Filtering those calls from historical messages rewrites Anthropic replay owners and loses error evidence. Anthropic tool search additionally requires authentic definitions for historical references. OpenAI supports request-local restrictions independently of historical messages.

## Options considered

1. Delete unavailable calls and results from outgoing history: small, but destroys evidence and violates exact replay ownership.
2. Keep every declaration executable: preserves history, but grants obsolete phase and service capabilities.
3. Retain declarations and history while enforcing current policy separately: supports both provider contracts and requires durable declaration and continuation state.

## Decision

Retain authentic conversation declarations separately from execution eligibility. Provider adapters render native restrictions only on verified routes; other routes receive advisory instructions and executor enforcement. Persist native Anthropic changes with source-message anchors, and retain full Responses output envelopes privately with their adopted assistant owner. Provider switches retire continuation state atomically rather than keeping suspended provider sessions.

Context management that removes native event anchors starts a fresh native tool prefix from current policy. Required private replay owners remain exact; Anthropic's explicit prefix-mismatch policy handles changed binding.

Migration creates empty declaration stores for existing conversations. Subsequent requests capture authentic catalog definitions; no historical schema is inferred from call arguments. An already-stuck exchange whose required definition was never captured needs an authentic catalog source before it can resume.

## Consequences

- **Positive:** transient MCP failures and phase changes preserve call evidence and private continuation, with execution permission checked independently.
- **Negative:** retained schemas and opaque continuation envelopes consume durable storage, and existing uncaptured schemas cannot be automatically recovered.
- **Neutral:** unknown compatible routes use advisory restrictions until native support is established; this does not grant execution authority.

## References

- [LLM requirements](../llm/requirements.md)
- [Shared Allium contracts](../llm/llm.allium)
- [Anthropic tool changes](https://platform.claude.com/docs/en/build-with-claude/mid-conversation-system-messages)
- [OpenAI function calling](https://developers.openai.com/api/docs/guides/function-calling)
- `Database::prepare_tool_availability`, `Database::update_state_and_replay`, `execute_tool_to_outcome`
