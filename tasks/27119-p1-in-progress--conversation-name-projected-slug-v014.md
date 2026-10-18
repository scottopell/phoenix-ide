# Deliver one conversation name and its projected slug for v0.14

## Scope and authority

Deliver on `feat/conversation-name-slug` for v0.14, not as a v0.13 release gate. No production deployment, public release, incident-controller change, or merge is authorized by this task. Source edits, lightweight validation, commits, pushes, PR creation and normal hosted qualification are authorized. Heavy local Rust builds/browser runtime need host admission.

Primary user design evidence was retrieved read-only from transcript `b9e54e2d-e8ec-472b-9e8c-40d201731eb5`:

- `@transcript:b9e54e2d-e8ec-472b-9e8c-40d201731eb5#message-b9e54e2d-e8ec-472b-9e8c-40d201731eb5:12409bab-a8d9-4e8e-95a5-46c6b8db3c99`: “One editable conversation name - supports spaces and full words, in URL default is slug-ified projection of that conversation name. no need to support old slugs as urls.”
- `@transcript:b9e54e2d-e8ec-472b-9e8c-40d201731eb5#message-b9e54e2d-e8ec-472b-9e8c-40d201731eb5:d6011abe-1cbb-4f54-b23d-f1f9c565dd47`: “yes absolutely, I view slug as projection of name (mental model)”; agreed readable slug + permanent product UUID + exact references, duplicate names with deterministic collision suffixes.

The Global implementation commission was relayed internally in message `b9e54e2d-e8ec-472b-9e8c-40d201731eb5:1b6c5515-0e0a-4848-93d1-5375a6bf6f0a`. It reports the user commissioning delivery for 0.14. This relay is not primary user evidence; the local design choices above are primary.

## Acceptance

- One editable human name; root title is authority, root slug is its automatically allocated projection. Preserve spaces/case/Unicode; allow duplicate names.
- Atomic deterministic slug allocation against slugs and immutable identities. Current readable `/c/<slug>` route, permanent `/c/<product UUID>`, exact transcript/message references unchanged.
- Migration 129 folds effective legacy names without message/identity loss; removes competing persisted chain-name authority. No broad schema rename or short-ID convention.
- Creation, rename, compatibility rename, regeneration and publication converge on the same authority and admission. Async work pins immutable identity.
- UI presentation, rename and new navigation use authoritative display name/current route while retaining stable cache/API identities and exact selection/hash.
- Retain legacy route consumers initially; stop new legacy route producers. Do not rewrite historic citations.

## Delivery and validation

Source implementation is delivered in PR #951, including backend/spec/UI integration, genuine hosted type generation, and independent review. The task remains in progress pending qualified integration; this task does not authorize merge or rollout. Actual exact-head checks and final review evidence are tracked on the PR and in `specs/conversation-identity/executive.md`. Local TypeScript/lint/Vitest and the integrated compiler-free harness suite (1,056 tests, one skipped) pass. Focused hosted workflow/naming regressions pass (67 tests); the final full hosted rerun remains authoritative. No local heavy Rust build, codegen build, or browser run was admitted. Migration 129 is not part of the qualified migration-128 incident target.
