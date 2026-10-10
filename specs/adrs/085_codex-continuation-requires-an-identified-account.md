# ADR-085: Codex continuation requires an identified account

- **Status:** Accepted
- **Date:** 2026-10-07
- **Affects:** REQ-LLM-004h, REQ-LLM-004i; AccountBoundCodexCredential, ConversationProviderContext

## Context

Private Responses envelopes and transport continuation belong to the account that produced them. Endpoint and model identity alone cannot distinguish an account switch. Credentials without account metadata can load, and the earlier admission path also permits constructing a Codex service from them. Token refresh changes bearer credentials without changing account ownership.

## Options considered

1. Keep ADR-062's loaded-credential prerequisite without requiring account metadata: preserves accountless service admission, but cannot establish continuation ownership.
2. Rotate continuation identity on every credential refresh or service construction: avoids unknown ownership, but unnecessarily discards same-account continuation and cannot provide durable identity across restart.
3. Require an identified account for Codex service construction and include that account in route identity: preserves same-account continuation and gives account switches a clear retirement boundary.

## Decision

Use identified-account admission. Account-bound credentials structurally require an account ID; startup and reload withhold Codex services when it is absent. Raw credential loading remains independent. Route identity includes the non-secret pinned account ID and excludes bearer and refresh tokens. A changed account retires continuation through the existing atomic route-change transaction before provider I/O.

This supersedes ADR-062's credential prerequisite. Its discovery policy remains: provider model listing is advisory for explicitly supported built-ins and never introduces unknown models. A known account's failed or incomplete listing does not deny those built-ins. Execution errors do not select another account, route, model, or billing identity. Direct API availability remains independent.

## Consequences

- **Positive:** Account switching cannot reuse another account's private replay; same-account token refresh preserves continuity.
- **Negative:** Accountless Codex credentials cannot admit bridge services until account identity is available.
- **Neutral:** Provider-catalog membership remains advisory; direct API and raw credential loading retain their own contracts.

## References

- [ADR-062](062_codex-catalog-discovery-is-advisory-for-builtins.md)
- [ADR-084](084_tool-policy-is-independent-of-provider-history.md)
- [LLM requirements](../llm/requirements.md)
- `AccountBoundCodexCredential::new`, `ModelRegistry::reload_codex_credential_snapshot`, `LlmServiceImpl::continuation_route_key`
