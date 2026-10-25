# Implement trusted-device Phoenix federation end to end

Implement the user-authorized bounded federation stack without merge, deployment, live migration, enrollment, credential changes, or production pairing.

## Coverage ledger

| Surface | Source status | Remaining gate |
|---|---|---|
| Stable identity / enrollment / verified HTTPS | Landed | Real two-host acceptance remains a separate Global-owned operational gate. |
| `query_database` | Committed | Shared omission-only `instance` selector and no-local-fallback regressions exist; exact-head hosted qualification remains. |
| `search_conversations` | Committed, review hardening dirty | Qualified destination evidence, foreign sender provenance, deleted-source hits, and source-call locators are implemented. |
| `read_conversation` | Committed, review hardening dirty | Stable-current/exact paging is implemented; current source binds navigation to resolved exact identity. |
| `resolve_reference` | Committed | Canonical destination navigation, exact evidence, typed provenance, and path owner identity are implemented. |
| `send_conversation_message` | Committed foundation, review split dirty | Public peer send is sender-free `RemoteApi`; a distinct fixed server-authored endpoint carries source-derived ordinary/Coordinator provenance. |
| Durable sender provenance | Committed foundation | Migration 130 normalizes remote instance identity across messages, steering, turns, retrieval, restart, and replay. Generated TS and final hosted tests remain. |
| Coordinator WorkScope | Delivered by separate owner | Task 98020 / REQ-GR-016–018 own this; federation verifies rather than duplicates it. |
| Python client | Committed | `752a1491f` adds typed remote operations; 40 focused tests pass locally. Server owner facades still need hosted Rust qualification. |
| Destination navigation | Source implemented | Destination-generated canonical HTTPS navigation remains separate from UUID identity and exact transcript/message evidence. |
| Federation OpenAPI | Dirty, unqualified | Schemars-generated Federation API reference and bounded saved-peer fetch exist; it is not claimed as the complete Phoenix API. |
| Opaque peer notes | Dirty and held | One bounded opaque value/API/UI exists. Goals owns migration 132; notes must become migration 133 after qualified goals source is incorporated. |
| Bounded terminal-fact subscriptions | Committed foundation plus dirty repairs | Migration 131, finite replay/authorization, retention/gaps, End-on-Close, durable admission, unsubscribe/revocation, presentation, and two-instance test source exist. Runtime acceptance is not yet qualified. |
| Native/web presentation | Source implemented | Remote origins and federated fact/gap cards preserve instance identity and never generate receiver-local links. Generated bindings remain a gate. |
| Automated acceptance | Extensive source; not qualified | File-backed two-instance HTTPS fixtures cover the five operations, subscriptions, TLS/auth/navigation/provenance/failure paths. Test presence is not execution evidence. |

## Published review ledger

| Finding | Disposition | Evidence gate |
|---|---|---|
| [Deleted-source search hit was dropped when navigation was absent](https://github.com/scottopell/phoenix-ide/pull/902#discussion_r2892613024) | Fixed in dirty source: typed evidence/sender survives `source_navigation: None`. | Rust/Python deleted-source regressions; hosted qualification pending. |
| [Stable sender lookup could erase exact historical provenance](https://github.com/scottopell/phoenix-ide/pull/902#discussion_r2892613353) | Fixed: stable not-found may be omitted while exact transcript/source call remains; other DB errors surface. | Deleted sender and DB-error regressions; hosted pending. |
| [UUID-shaped peer names were unreachable](https://github.com/scottopell/phoenix-ide/pull/902#discussion_r2892616804) | Fixed by rejecting UUID-shaped display names at issue/import/DB boundaries. | Import/overwrite/selector tests; hosted pending. |
| [Search DTO discarded the recorded source tool call](https://github.com/scottopell/phoenix-ide/pull/902#discussion_r2892617297) | Fixed with bounded complete `SourceToolCall` on qualified sender DTOs. | Serde/OpenAPI/local/remote tests; generated binding and hosted gates pending. |
| [Peer send body allowed sender-role forgery](https://github.com/scottopell/phoenix-ide/pull/902#discussion_r2893016421) | Public peer send now has no sender field and always records `RemoteApi`; a separate server-authored operation is used by the source-derived tool path. REQ-FED-006 trusts the enrolled source server and does not claim independent signed authorship. | Closed-body/direct API/authored-tool regressions; hosted pending. |
| [Unsubscribe could admit an already-fetched notification](https://github.com/scottopell/phoenix-ide/pull/902#discussion_r2893017566) | Fixed: local stop is checked before drain and suppresses every nonaccepted staged/authorizing row. | Barrier ordering tests; hosted pending. |
| [All-hidden read batches could fail to advance](https://github.com/scottopell/phoenix-ide/pull/902#discussion_r2893018426) | Obsolete in the sequence-ID implementation; cursor advancement is based on scanned sequence before filtering. | Added all-hidden multi-batch termination/monotonic-cursor regressions. |
| [Revoked credentials could replay an authorization receipt](https://github.com/scottopell/phoenix-ide/pull/902#discussion_r2893054774) | Fixed: peer middleware always requires an active verifier; revoked replay is HTTP 401. | Revoked authorized-receipt HTTPS regression; hosted pending. |
| [Create/replay 401 did not update local lifecycle](https://github.com/scottopell/phoenix-ide/pull/902#discussion_r2893055097) | Fixed: active create/replay auth rejection durably ends the exact local subscription as `Revoked`. | Empty-pending replay and in-flight create revocation tests; hosted pending. |

Identity assessment [#951 comment 6267624980](https://github.com/scottopell/phoenix-ide/pull/951#issuecomment-6267624980) established that #951 does not solve remote sender provenance and that `canonical_route` is navigation only. The branch preserves ProductConversation UUID identity/cache keys, exact transcript/message evidence, and destination-owned canonical navigation separately.

## ADR and migration coordination

- Provisional federation ADR numbers collided with accepted main decisions. The bounded-subscription decision is ADR-180; the explicit End-on-Close lifecycle is ADR-181.
- Goals owns ADR-182 and migration 132.
- Federation migrations 130/131 are immutable source dependencies.
- Peer notes remain held for migration 133; no conflicting 132 is publishable.
- No published migration has been renumbered or rewritten.

## Verification ledger

- `39178832713` at `b98cba6c…`: terminal failure; three Clippy findings plus incorrect WorkScope-owner test identified.
- `39266332096` at `d5f4c518…`: provenance/send source compiled into tests and exposed runtime failures plus reserved-opening fixture and binding lint.
- `866e5e56d`: published missing subscription presentation module, binding lint fix, and immutable-row fixture correction for non-destructive import by goals.
- `39289109607` at `866e5e56d…`: task validation passed; Rust exposed exact migration/subscription test compile errors.
- `004e3ed76`: published the exact compile corrections from that run without changing migrations or goals source.
- `39293294676` at `004e3ed76…`: exact hosted run dispatched; terminal result must be recorded before acceptance.
- Generated `InputOrigin` and `EventPresentation` TypeScript are stale. They must come from the approved hosted codegen workflow; generated files will not be edited manually or locally compiled without host admission.
- Current main also has independent blockers: the migration-128 baseline/spec-shape drift and `Effect` large-enum Clippy. They are neither federation success evidence nor silently waived.

## Remaining work before source merge

1. Incorporate qualified goals migration 132, then register notes as migration 133.
2. Publish the current review/auth/owner-facade/OpenAPI/notes/subscription repairs as coherent commits.
3. Obtain hosted generated TypeScript from the exact pushed source.
4. Drive exact-head Rust, Clippy, UI/spec, E2E and native checks to terminal outcomes; fix introduced failures and classify independent main blockers precisely.
5. Complete final independent review and full isolated two-instance acceptance.
6. Keep merge, deployment, live migration, enrollment and real two-host acceptance as separate explicitly authorized gates.

## Scope exclusions

Do not add database replication, remote Bash/process control, generic RPC, RBAC/multitenancy, broadcast discovery, indefinite event streaming, transcript/token streaming, a distributed scheduler, insecure TLS fallback, or production enrollment/configuration. Global retains publication, deployment and live pairing.
