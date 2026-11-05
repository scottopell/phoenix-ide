# Federation — Executive Status

## Summary

Phoenix has saved-peer enrollment, authenticated peer reads, optional-instance conversation read/resolve support, and optional-instance `send_conversation_message`, `read_goal`, and Coordinator `query_database` routing through destination-owned admission. All five optional-instance tools share a typed selector: omission and exact `"local"` use local services; read tools also treat explicit `null` as local, while other strings retain remote selection. Ordinary non-Coordinator query registrations remain local-only. Durable remote watches remain in progress. Existing subscription groundwork does not by itself establish watch behavior.

## Requirement status

| Requirement | Status | Current evidence |
|---|---|---|
| REQ-FED-001–003 | Implemented | Shared local/remote selection covers send, read, resolve, Goal read, and Coordinator database query. Tool-level regressions cover authority-specific schemas, local parity, delivery/steering replay and provenance, target and Coordinator restrictions, local paging/message context, Goal waits/history, UUID/name remote routing, and retained remote failure behavior without local fallback. |
| REQ-FED-004–006 | Implemented | Authenticated remote attribution, peer-scoped durable retry identity, destination admission, direct/steering replay, revocation/replacement rechecks, cancellation survival, and database/runtime reconstruction are covered by focused tests. |
| REQ-FED-007 | Implemented | Optional-instance Goal reads preserve stable resolution, version waiting, timeout, result semantics, and malformed-response rejection. |
| REQ-FED-008 | Partial | Remote message and Goal-read results identify the stable ProductConversation and exact admitted transcript; remote watch results remain. |
| REQ-FED-009–016 | Partial | Durable subscription schema and local Coordinator notification admission are groundwork only; remote watch creation, delivery, replay, bounds, close/gap recovery, unsubscribe, and revocation behavior remain. |
| REQ-FED-017–018 | Defined | Complexity boundaries and non-goals are normative in `requirements.md`. |
| REQ-FED-019 | Implemented | Enrollment and peer-connection writes require a validated `PeerDisplayName`; constructor/JSON and HTTP tests pass for blank names and case/whitespace variants of local, including rejection without replacing credentials or connections. |

## Existing foundations

- Saved peer identity, strict TLS trust, bearer authentication, and destination identity verification.
- Shared `InstanceSelector` reserves exact `"local"` before peer lookup without changing saved peers or local-only ordinary tools. Its property test preserves every non-local string as a remote selector; authority-specific query schemas and tool tests preserve null/type validation.
- New enrollment and saved-connection names use `PeerDisplayName`, which reserves local independent of ASCII case and surrounding whitespace. Saved labels are not rewritten by selection.
- Unified local or remote Coordinator database query, stable-reference resolution, and conversation read/pagination.
- Local payload-bound message replay through `message_id`.
- Optional-instance remote message routing with authenticated source attribution, stable and exact destination receipts, durable peer-scoped replay, and no local fallback.
- Local and optional-instance Goal reads with stable resolution, version waiting, timeout, typed wait outcomes, exact transcript provenance, and malformed-response rejection.
- Local durable Coordinator watch notification admission and deduplication.
- Federation event, source-subscription, and consumer-state tables.

## Verification still required

- Full repository validation and an actual accepted mutation through a second Phoenix process or machine; focused local persistence, API, runtime, Coordinator-tool, codegen, and UI checks do not satisfy that acceptance.
- Watch replay, local durable admission before acknowledgement, restart recovery, bounded backlog failure, gap resynchronization, ordered unsubscribe, and revocation-admission race tests.

## Exclusions

This slice does not implement Goal mutations (including creation), settings, Continue, conversation creation or rename, reverse enrollment, deployment, or live peer changes.
