# Investigate generic OpenAI Platform service-tier support

## Problem

Phoenix's OpenAI Responses request types do not represent the platform `service_tier` field. The Codex fast-mode task deliberately supports only ChatGPT/Codex `priority` routing for catalog-advertised models. Operators using an OpenAI API key may separately want Priority processing, Flex processing, or standard/default processing.

This task must use current OpenAI API documentation as the platform authority; `codex-rs` remains useful implementation evidence but is not authoritative for API-key billing, eligibility, or fallback semantics.

## Scope

1. Verify the current Responses API contract for `service_tier`, including accepted values, account/project eligibility, pricing/usage consequences, response fields, fallback behavior, and whether Priority and Flex share one capability model.
2. Decide the product surface separately from Codex fast mode:
   - operator configuration versus per-conversation selection;
   - explicit Standard/Priority/Flex typed choices versus a smaller supported subset;
   - whether provider-returned effective tier must be recorded and displayed;
   - behavior when an account is not eligible or capacity causes fallback.
3. Add route-aware capability metadata. Do not advertise Codex catalog capability as proof of Platform API entitlement, and do not expose tier controls for arbitrary OpenAI-compatible external providers unless their contract is explicitly configured.
4. If approved by the verified API contract, persist the typed selection relationally, validate it atomically with model/provider changes, and translate it to the native Responses request field. Standard/default must have one canonical wire representation.
5. Keep HTTP and WebSocket continuation compatibility correct when the requested tier changes.
6. Add usage disclosure, observability for requested/effective tier, and provider-error handling without silently changing a user's paid routing choice.

## Acceptance evidence

- Research notes cite current official OpenAI documentation and distinguish verified API behavior from assumptions based on Codex.
- Product decisions above are resolved before implementation; unsupported combinations are structurally unrepresentable.
- Golden wire tests cover each supported Platform tier and omission/default behavior without changing Codex bridge requests.
- Capability/API tests prove controls appear only on eligible Platform routes and not merely because a model has Codex fast support.
- Persistence and model-switch tests cover atomic validation and safe reset behavior.
- Continuation tests prove tier changes cannot reuse an incompatible prior response.
- Usage/telemetry tests distinguish requested tier from any provider-reported effective tier.
- `./dev.py check` passes.

## Non-goals

- Codex fast mode, tracked in task 24708.
- Importing or mirroring the Codex model catalog.
- Passing arbitrary unvalidated service-tier strings to third-party OpenAI-compatible endpoints.
- Treating reasoning effort or model choice as a service tier.
