# ADR-118: Public literal chat uses a dedicated fail-closed route

- **Status:** Accepted
- **Date:** 2026-10-17
- **Affects:** REQ-API-030, REQ-COMP-015; built-in `PhoenixApiDestinationAuthority` guidance

## Context

The shared chat application service already supports literal input for internal callers, but public chat expands references. User-authorized API steering needs a small public literal contract without exposing generated predecessor context or bypassing authentication, attachment validation, or lifecycle admission. The user selected the bounded endpoint option with fail-closed capability discovery.

A version preflight followed by a request field on `/chat` is insufficient: a receiver replaced between requests can ignore an unfamiliar field and expand the text. Matching transcript text alone also does not prove which expansion policy was accepted historically.

## Options considered

1. **Policy field on `/chat` only, with a version capability** — minimal route surface, but historical receivers can silently ignore the field after preflight.
2. **Dedicated literal route sharing the existing service, plus exact capability** — one small handler and route; receivers lacking that route cannot dispatch through the expanding endpoint.
3. **General protocol negotiation or receiver-version fencing** — stronger live-replacement machinery with substantially broader lifecycle and compatibility obligations than this commissioned feature needs.

## Decision

Add the closed public `ChatExpansionPolicy` enum (`expand_references`, `literal_text`) with the ordinary `/chat` default unchanged. Add `POST /api/conversations/{id}/chat/literal`, accepting `ChatRequest` but requiring explicit `literal_text` before invoking the existing authenticated/admission-locked send path. Omitted/default policy fails with 400; invalid enum values fail decoding.

Advertise `chat_literal_text_v1: true` on `/api/version` for this exact dedicated-route contract. Guidance requires complete HTTP-200 JSON with that exact boolean from the same authenticated verified-TLS destination before each POST or exact retry, no redirects, and only `/chat/literal` with explicit literal policy. Never fall back to `/chat`, including after a route refusal or capability loss. Retain the original transcript, once-generated message identity and exact intent for reconciliation.

Require durable policy evidence for nondefault replay. Direct-turn receipts and exact steering fingerprints remain authoritative; transcript-only historical evidence can replay only default expanding intent. No new receipt framework or database migration is added.

## Consequences

- **Positive:** Public literal idle and steering sends reuse existing authorization, admission, attachment handling, provenance and idempotency. Historical receivers cannot silently reinterpret the dedicated route as expanding chat.
- **Negative:** Literal callers must perform discovery and use the dedicated route; unsupported destinations stop instead of degrading. Literal retries against receipt-free historical messages conflict even when visible text matches.
- **Neutral:** The optional policy on ordinary `/chat` is not advertised as a safe cross-version recipe. This adds no database downgrade, mixed-version ownership, deployment or live-replacement guarantee, and changes no local-only tool or destination-authority boundary.

## References

- [API requirements](../api/requirements.md), REQ-API-030
- [Compatibility requirements](../compatibility/requirements.md), REQ-COMP-015 and REQ-COMP-001
- [Built-in guidance](../builtin-skills/builtin-skills.allium), `PhoenixApiDestinationAuthority`
- ADR-034 (explicit compatibility policy), ADR-113 (non-exhaustive API recipes)
- `ChatRequest`, `ChatExpansionPolicy`, `send_literal_chat`, `send_chat`, `version_json_payload`, `SendChatApplicationService`, `lookup_durable_steering_replay`
