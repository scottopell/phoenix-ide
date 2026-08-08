## Outcome

Complete the native iOS conversation state and action policy layer on top of the offline-first client foundation.

## Scope

- Add typed conversation lifecycle states and derive visible actions from server-owned state.
- Keep transcript, conversation-list, archive, delete, stop, task-approval, and offline/error UI mutually consistent.
- Propagate authoritative conversation metadata through SSE and refresh paths.
- Cover the policy and refresh behavior with focused unit tests.

## Primary paths

- `ios/PhoenixClient/App/`
- `ios/PhoenixClient/Networking/`
- `ios/PhoenixClient/Views/`
- `ios/PhoenixClientTests/`
- `specs/ios_client/`

## Acceptance

- Action affordances are derived from typed lifecycle/capability state; invalid actions are not presented.
- Archive, delete, stop, approval, reconnect, and refresh flows remain correct across offline transitions.
- Conversation title/state updates reach both the open transcript and conversation list.
- Focused iOS unit tests and the repository checks pass.

## Dependency

Starts after the offline-first iOS core PR is merged.

## Out of scope

Image attachments, coordinator setup, background refresh, push delivery, and comprehensive renderer polish.
