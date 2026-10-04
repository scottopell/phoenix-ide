# Make Codex catalog discovery advisory for supported built-in models

## Observed journey

- A user with a configured Phoenix ChatGPT/Codex credential can successfully execute the exact built-in `gpt-6-sol` model, yet Phoenix omits it from the available model registry when the provider's model-list response omits that slug.
- The reported deployed revision is `ef75f99` (current `origin/main`, including #801 and #808). Provider/CLI catalogs from the same account omitted GPT-6 Sol/Luna while one explicitly authorized, tool-free exact GPT-6 Sol request succeeded. This proves a Sol false negative only; it is not evidence of Luna entitlement or broad tool/effort support. No additional live or paid probes are authorized or needed.

## Verified findings

- `ModelRegistry::codex_catalog_allows` on `origin/main` returns false for `CodexAvailability::AccountCatalog` when discovery fails or the exact API name is absent. `try_create_model_with_codex_catalog` therefore withholds those built-ins during startup, and `reload_codex_credential_snapshot` applies the same gate during credential reload.
- `gpt-6-astra`, `gpt-6-sol`, and `gpt-6-luna` are marked `AccountCatalog`; focused tests explicitly require discovered membership. Established GPT-5.6 built-ins already demonstrate catalog-independent registration.
- REQ-LLM-004h currently makes exact account-catalog membership denial authority, so the requested policy change requires a normative update and decision record, plus an executive-status correction.
- Discovery is bounded to known built-in/operator-configured specs; unknown listed slugs are not auto-registered. Built-in Codex routing is also separately gated by configured bridge intent, loaded credentials, OpenAI Responses backend, and built-in source.
- Credential reload separately snapshots and validates credential/account identity around asynchronous discovery, preserves prior state on unsafe reload failures, binds services to the validated account, and atomically publishes service/spec/credential state. Those safeguards do not depend on catalog membership being denial authority and must remain.
- Existing exact legacy-pin mappings, custom-route precedence, route-specific effort/capability metadata, and exact manual parallel-Work qualification are independent policies that must not broaden.

## Inferences and unknowns

- The smallest correction is to treat Codex listing results as advisory observations for explicitly supported built-ins while retaining connection/credential presence as the availability prerequisite. The exact internal type/name may change so it cannot continue to encode the obsolete hard-gate policy.
- Actual account entitlement cannot be guaranteed at listing time. A provider rejection during execution must surface as the honest error for the selected model/account; it must not trigger model, account, billing-route, or default-model substitution.
- No product question remains: the user has explicitly selected advisory discovery for supported built-ins and fail-closed behavior for unknown identifiers and routing identity.

## Interaction map

- Built-in `ModelSpec` and configured custom specs → registry route/auth eligibility → startup registry publication → model picker/runtime resolution.
- Credential file and account identity → asynchronous Codex discovery → identity revalidation/generation fence → atomic credential-reload publication → account-bound request credential.
- Resolved exact model → route-specific protocol/capabilities/effort and manual orchestration qualification → provider execution → exact provider error on unavailability.

## Proposed scope

Change the normative and implementation policy so a configured, loaded ChatGPT/Codex connection registers Phoenix-supported built-in OpenAI Responses models even when account catalog discovery fails or omits their exact slugs. Provider listing remains advisory and must never introduce unsupported models.

Likely focused surfaces:

- `crates/phoenix-llm/src/models.rs`: replace or narrow the obsolete per-built-in hard-gate representation while preserving source, protocol, capability, and effort metadata.
- `crates/phoenix-llm/src/registry.rs`: apply the advisory policy consistently at startup and credential reload without bypassing bridge intent, credential/account binding, discovery identity validation, generation fencing, safe publication, or atomic replacement.
- `specs/llm/requirements.md`, `specs/llm/executive.md`, and a new project ADR superseding the catalog-denial decision/rationale; keep timeless requirements free of rollout evidence and avoid claiming entitlement verification.
- Focused registry/model tests using existing local harnesses only.

Acceptance coverage:

1. At startup, a credentialed Codex connection registers supported built-ins when the discovered catalog omits them and when discovery fails/returns no catalog.
2. Credential reload does the same for first login, same-account refresh, and account switch while preserving account-bound route identity and the existing validation/publication fences.
3. Missing connection intent or missing/invalid credentials does not expose a Codex route or silently fall through to an API-key/billing route.
4. Unknown provider-listed identifiers remain unregistered; custom configured identities/routes retain their current behavior and precedence.
5. Existing source identity, Responses Lite/WebSocket routing, route-native effort/capability validation, legacy pinned-model retirement mappings, default-selection semantics, and explicit manual orchestration qualification remain exact and do not infer support from family/version/catalog entries.
6. An unavailable selected model yields the provider's model error with no model/account/billing fallback, automatic probe, or broad configuration rewrite; cover this with the existing service/registry harness rather than live integration.
7. Focused crate tests and `./dev.py check` pass; inspect the final diff for accidental policy broadening.

## Explicit non-goals

- No further live/paid probes, credential inspection/dumps, production mutation, deployment, restart, or conversation/configuration changes.
- No provider-catalog auto-registration, entitlement guarantee, Luna validation claim, capability inference, orchestration-qualification expansion, fallback framework, or broad discovery/configuration refactor.
- No changes to unrelated flake or user-design streams.

## Delivery

After approval, integrate current `origin/main`, implement and test the bounded change, commit and push one PR, request Codex review against the exact HEAD, read and converge findings plus advancing-cursor review/CI, then provide a qualified landing handoff. Do not merge, deploy, restart, or mutate production.
