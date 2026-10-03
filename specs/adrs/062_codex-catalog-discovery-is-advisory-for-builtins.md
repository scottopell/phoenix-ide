# ADR-062: Codex catalog discovery is advisory for supported built-ins

- **Status:** Accepted
- **Date:** 2026-09-27
- **Affects:** REQ-LLM-003, REQ-LLM-004h; `ModelRegistry`

## Context

Phoenix has an explicit built-in model registry that defines the exact model identifiers, protocols, route capabilities, and reasoning-effort values it supports. ChatGPT/Codex also exposes account-scoped model discovery, but that listing can omit an exact model that the same account can execute. Treating listing membership as denial authority therefore creates false unavailability.

Catalog discovery still crosses an account boundary. Its result must remain bound to the credential identity that produced it, and unknown discovered identifiers must not expand Phoenix's supported model or orchestration catalogs.

## Options considered

1. **Require exact provider-catalog membership** — avoids advertising omitted models, but falsely denies supported executable models when listing is incomplete.
2. **Probe omitted models automatically** — could distinguish some entitlements, but spends requests, introduces side effects, and still cannot establish every capability combination.
3. **Treat discovery as advisory for explicit built-ins** — uses Phoenix's registry as support authority, keeps connection credentials as the route prerequisite, and lets actual execution report account-specific unavailability.

## Decision

Provider model discovery is advisory for Phoenix-supported built-in Codex models. A configured Codex connection with a loaded credential registers those built-ins whether discovery succeeds, fails, or omits an exact identifier.

Discovery does not add models to Phoenix's registry or infer model capabilities, reasoning effort, pricing, protocol support, or parallel-Work qualification. If execution rejects a selected model, Phoenix returns that provider error without substituting another model, credential, account, authentication route, or billing route.

Credential snapshot validation, account-identity binding, generation fencing, and atomic reload publication remain required even though catalog membership is not an availability gate.

## Consequences

- **Positive:** Incomplete provider listings no longer hide Phoenix-supported built-ins.
- **Positive:** Unknown catalog entries remain unusable until explicitly added to Phoenix's built-in or operator-configured registry.
- **Positive:** Account and billing identity cannot change as an implicit recovery path.
- **Negative:** A picker can expose a supported built-in that a particular account cannot execute; the provider's execution error is the authoritative outcome.
- **Neutral:** Direct API routes, custom configured identities, capability validation, legacy pin mappings, and manual orchestration qualification are unchanged.

## References

- `specs/llm/requirements.md`
- `ModelRegistry::new_with_codex_catalog`
- `ModelRegistry::reload_codex_credential`
- `AccountBoundCodexCredential`
