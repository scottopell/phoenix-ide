# Federation — Executive Status

## Summary

Phoenix has saved-peer enrollment, authenticated peer reads, optional-instance conversation read/resolve support, and optional-instance `send_conversation_message` routing through the destination's ordinary admission path. Optional-instance `read_goal` and durable remote watches remain in progress. Existing subscription groundwork does not by itself establish watch behavior.

## Requirement status

| Requirement | Status | Current evidence |
|---|---|---|
| REQ-FED-001–003 | Partial | Saved-peer selection, authenticated routes, and optional-instance message routing are implemented without local fallback; optional-instance Goal routing remains. |
| REQ-FED-004–006 | Implemented | Authenticated remote attribution, peer-scoped durable retry identity, destination admission, direct/steering replay, revocation/replacement rechecks, cancellation survival, and database/runtime reconstruction are covered by focused tests. |
| REQ-FED-007 | In progress | Local Goal reads exist; optional-instance routing with stable resolution, version waiting, timeout, and remote-result parity remains. |
| REQ-FED-008 | Partial | Remote message receipts identify the stable ProductConversation and exact admitted transcript; remote Goal-read and watch results remain. |
| REQ-FED-009–016 | Partial | Durable subscription schema and local Coordinator notification admission are groundwork only; remote watch creation, delivery, replay, bounds, close/gap recovery, unsubscribe, and revocation behavior remain. |
| REQ-FED-017–018 | Defined | Complexity boundaries and non-goals are normative in `requirements.md`. |

## Existing foundations

- Saved peer identity, strict TLS trust, bearer authentication, and destination identity verification.
- Remote SQL, stable-reference resolution, and conversation read/pagination.
- Local payload-bound message replay through `message_id`.
- Optional-instance remote message routing with authenticated source attribution, stable and exact destination receipts, durable peer-scoped replay, and no local fallback.
- Local Goal read with version waiting and timeout.
- Local durable Coordinator watch notification admission and deduplication.
- Federation event, source-subscription, and consumer-state tables.

## Verification still required

- Full repository validation and an actual accepted mutation through a second Phoenix process or machine; focused local persistence, API, runtime, Coordinator-tool, codegen, and UI checks do not satisfy that acceptance.
- Goal-read local/remote parity tests.
- Watch replay, local durable admission before acknowledgement, restart recovery, bounded backlog failure, gap resynchronization, ordered unsubscribe, and revocation-admission race tests.

## Exclusions

This slice does not implement Goal mutations (including creation), settings, Continue, conversation creation or rename, reverse enrollment, deployment, or live peer changes.
