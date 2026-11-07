# Federation — Executive Status

## Summary

Phoenix has saved-peer enrollment, authenticated peer reads, optional-instance conversation read/resolve support, and optional-instance `send_conversation_message`, `read_goal`, Coordinator `query_database`, and Coordinator `read_automatic_continuation`/`set_automatic_continuation` routing through destination-owned admission. Omission and exact `"local"` use local services; the preference tools reject explicit `null` even on read, while other read tools treat it as local. Other strings select an exact saved peer without fallback. Ordinary non-Coordinator query registrations remain local-only. Durable remote watches provide destination-owned enrollment, continuation following, accepted-delivery deduplication, restart and bounded-gap recovery, ordered unsubscribe, and revocation-aware closure.

## Requirement status

| Requirement | Status | Current evidence |
|---|---|---|
| REQ-FED-001–003 | Implemented | Shared local/remote selection covers send, read, resolve, Goal read, and Coordinator database query. Tool-level regressions cover authority-specific schemas, local parity, delivery/steering replay and provenance, target and Coordinator restrictions, local paging/message context, Goal waits/history, UUID/name remote routing, and retained remote failure behavior without local fallback. |
| REQ-FED-004–006 | Implemented | Authenticated remote attribution, peer-scoped durable retry identity, destination admission, direct/steering replay, revocation/replacement rechecks, cancellation survival, and database/runtime reconstruction are covered by focused tests. |
| REQ-FED-007 | Implemented | Optional-instance Goal reads preserve stable resolution, version waiting, timeout, result semantics, and malformed-response rejection. |
| REQ-FED-008 | Implemented | Remote message, Goal-read, and watch results preserve stable ProductConversation identity and exact admitted transcript provenance. |
| REQ-FED-009–016 | Implemented | Two-instance and persistence regressions cover destination-owned future-only enrollment, idle and busy admission, Continue-chain following, accepted-delivery deduplication, restart recovery, bounded-gap evidence and replay, ordered online/offline unsubscribe, late-create fencing, source closure, and revocation/replacement races. |
| REQ-FED-017–018 | Defined | Complexity boundaries and non-goals are normative in `requirements.md`. |
| REQ-FED-019 | Implemented | Enrollment and peer-connection writes require a validated `PeerDisplayName`; constructor/JSON and HTTP tests pass for blank names and case/whitespace variants of local, including rejection without replacing credentials or connections. |
| REQ-FED-020 | Implemented; deployment pending | Separate closed remote read/set requests, source Coordinator dispatch gate, authenticated destination transaction, strict preference-only set and response verification passed focused regressions, exact-range review, full repository qualification, and merge-head CI. Production deployment and live cross-host acceptance are not established. No generic settings, trust/enrollment, or combined enable-and-create operation is included. |
| REQ-FED-021 | In progress (specification) | Remote Coordinator Goal prepare/apply specifies source-boundary presence-aware selection, typed enrollment and source-tool-call provenance, destination Coordinator `ProductConversation` targeting and destination-derived expected version, reference-only federation apply with internal preparation lookup, and one retained enrollment-scoped terminal receipt replayed before expiry/version checks under current authority gates. Local Goal tooling exists; remote mutation is not claimed as implemented or runtime-verified. |

## Existing foundations

- Saved peer identity, strict TLS trust, bearer authentication, and destination identity verification.
- Shared `InstanceSelector` reserves exact `"local"` before peer lookup without changing saved peers or local-only ordinary tools. Its property test preserves every non-local string as a remote selector; authority-specific query schemas and tool tests preserve null/type validation.
- New enrollment and saved-connection names use `PeerDisplayName`, which reserves local independent of ASCII case and surrounding whitespace. Saved labels are not rewritten by selection.
- Unified local or remote Coordinator database query, stable-reference resolution, and conversation read/pagination.
- Local payload-bound message replay through `message_id`.
- Optional-instance remote message routing with authenticated source attribution, stable and exact destination receipts, durable peer-scoped replay, and no local fallback.
- Local and optional-instance Goal reads with stable resolution, version waiting, timeout, typed wait outcomes, exact transcript provenance, and malformed-response rejection.
- Durable Coordinator watches with destination-owned admission, source continuation following, accepted-delivery deduplication, restart and bounded-gap recovery, ordered unsubscribe, and revocation-aware closure.
- Federation event, source-subscription, consumer-state, gap-evidence, and tombstone persistence.

## Verification still required

- Production rollout and live cross-host acceptance of the automatic-continuation controls remain unverified. Repository qualification and merge-head CI do not establish deployed behavior.
- Operational interoperability still requires compatible deployment on participating hosts; this does not reopen the implemented watch lifecycle.

## Delivery inventory

Local Goal tooling exists; remote Coordinator Goal mutation via prepare/apply is in progress at the specification stage, with no runtime qualification claimed. Goal routing selector presence belongs to the source tool boundary; the federation apply request itself contains only the opaque preparation reference, and lookup is destination-internal. Generic settings beyond the typed automatic-continuation preference, Continue, conversation creation, rename, reverse enrollment, live peer changes, and combined enable-and-create are not delivered under this federation contract. Conversation creation and rename remain excluded. This inventory reports delivered behavior; it does not narrow separately authorized scope.
