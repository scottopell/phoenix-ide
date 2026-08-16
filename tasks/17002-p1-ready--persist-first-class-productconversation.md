# Persist first-class ProductConversation identity and transcript membership

## User-visible outcome

A conversation retains one stable identity while continuation creates fresh transcript and execution segments. The aggregate, root segment, latest segment, subordinate execution participants, and Coordinator remain structurally distinct even when migrated identifiers happen to contain equal bytes.

## Authority

Implement the dormant aggregate-identity foundation required by ADR-031 and REQ-BED-030A. Current production persistence still infers aggregate identity from a root `conversations` row, and migration 64 still anchors Close obligations to that row. This task closes that foundation gap without activating Open/History lifecycle or live Close behavior.

## Scope

- Persist a first-class ProductConversation with an independently typed identity and typed `ordinary` or `coordinator` kind.
- Give every durable conversation transcript/execution row exactly one explicit ProductConversation membership reference.
- Deterministically backfill existing aggregates from continuation topology. Backfilled aggregate and root transcript identifiers may contain equal bytes but remain distinct identity and foreign-key domains.
- Allocate ProductConversation and initial transcript identities independently for new runtime creation; runtime code must never derive one identity from the other.
- Preserve membership through continuation, subordinate execution creation, Coordinator creation/recovery, and creation replay.
- Keep root and latest transcript identities derived from continuation topology; do not add mutable root/latest aggregate columns.
- Re-anchor dormant Close obligations to ProductConversation identity while retaining exact attempt-bound transcript-member snapshots.
- Expose typed domain/repository APIs that cannot accept a transcript identity where a ProductConversation identity is required.
- Keep ordinary lifecycle data dormant until the coordinated History cutover. Legacy `conversations.archived` remains the sole production lifecycle authority, receives no peer lifecycle dual-write, and no production reader may treat a dormant aggregate lifecycle projection as truth.

## Non-goals

- No live Close admission, settlement, inspection, WorkScope retirement, or repair orchestration.
- No History finalization, listing, deletion, or lifecycle authority cutover.
- No ProductConversation-to-WorkScope attachment table or attachment-authority migration.
- No Open/History API, route, SSE root ledger, UI, or client DTO.
- No proposal placement, follow-up/provenance, repository-authority generation activation, or legacy chain/mode/archive removal.
- No compatibility guarantee beyond the existing forward migration policy.

## Acceptance evidence

- Migration coverage includes an ordinary single-row conversation, a multi-segment continuation, subordinate execution participants, Coordinator, open and archived legacy rows, and dormant Close attempts.
- Schema/domain tests reject missing or cross-aggregate membership, cross-aggregate continuation, invalid Coordinator lifecycle shape, and transcript/ProductConversation identity substitution.
- New creation proves independent aggregate/transcript allocation; continuation and subordinate execution creation preserve aggregate membership.
- Close repository APIs bind obligations to ProductConversation identity and derive the exact parent-transcript topology snapshot.
- A production-consumer audit proves no reader treats dormant ProductConversation lifecycle as authority and no lifecycle dual-write was introduced.
- Focused migration, repository, creation, continuation, and restart tests pass, followed by `./dev.py check --all` and exact-head review.
