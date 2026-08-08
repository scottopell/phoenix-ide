## Outcome

Complete media, coordinator, and background lifecycle support for the native iOS client after the state/action layer lands.

## Scope

- Add image attachment selection, preview, upload, persistence, and resend behavior.
- Add coordinator discovery/setup, remembered selection, and server-switch invalidation.
- Add bounded background refresh and local notification behavior that respects sign-out and user opt-out.
- Add the highest-value end-to-end simulator journey and focused regression tests.

## Primary paths

- `ios/PhoenixMobile/Sources/`
- `ios/PhoenixMobile/Tests/`
- `ios/PhoenixMobile/UITests/`
- `specs/ios_client/`

## Acceptance

- A message cannot send before selected media has finished loading, and durable resend preserves media exactly once.
- Coordinator/background work cannot repopulate state after sign-out, server change, cache clear, or refresh opt-out.
- Background completion is signaled exactly once and delivered notifications follow archive/delete semantics.
- Focused iOS unit tests, the live simulator journey, and repository checks pass.

## Dependency

Starts after the iOS state/actions PR is merged.

## Out of scope

APNs delivery and the later comprehensive renderer, grounding panel, Markdown reader, and commenting-interface pass.
