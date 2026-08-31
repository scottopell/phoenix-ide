# ADR-031: ProductConversation persistence uses staged single authority

- **Status:** Accepted
- **Date:** 2026-08-10
- **Affects:** REQ-BED-029, REQ-BED-030, REQ-BED-030A, REQ-BED-031B, REQ-CHN-002, REQ-CHN-003, REQ-CHN-005, REQ-CHN-007, REQ-CHN-008, REQ-CHN-009, REQ-CHN-010, REQ-GR-001, REQ-GR-002, REQ-GR-005, REQ-GR-009, REQ-GR-011, REQ-PROJ-014, REQ-PROJ-015, REQ-PROJ-019, REQ-PROJ-WS-001, REQ-WL-001, REQ-WL-002, `ProductConversation`, `Conversation`, `CloseObligation`, `CloseAttemptMember`, `AttachedWorkScope`

## Context

ADR-026 separates ProductConversation lifecycle, transcript topology, and
`WorkScope` resource ownership. The normative model already treats them as
distinct concepts, but the persisted model still makes a root `conversations`
row do several jobs: its identifier stands in for ProductConversation identity,
its `archived` value approximates aggregate lifecycle, and row fields plus
continuation topology are used to infer aggregate membership and attached
resources.

The dormant Close foundation makes this mismatch costly. Its normalized attempt,
topology-snapshot, inspection, inventory, loss, and retirement-evidence records
are useful, but binding a Close obligation to the root transcript row would make
that row the durable owner again. Further trigger hardening cannot correct the
authority boundary while ProductConversation remains only an inference.

The persistence redesign must also be staged. Existing readers and writers still
use the legacy row model. Introducing a second writable lifecycle or attachment
representation before all consumers can move would create split-brain state,
while changing every lifecycle, project, and cleanup consumer in the dormant
foundation would pull later behavior into the wrong workstream.

## Options considered

1. **Keep the root transcript row as ProductConversation** — minimizes schema
   change, but preserves the identity/lifecycle conflation and binds aggregate
   obligations to an execution segment.
2. **Mint unrelated ProductConversation identifiers for all existing data** —
   creates clean-looking identifiers, but adds unnecessary remapping risk to a
   deterministic backfill and breaks stable references without improving the
   authority boundary.
3. **Dual-write legacy and normalized lifecycle** — allows incremental reader
   migration, but creates two writable representations whose disagreement has no
   structurally correct resolution.
4. **Add a writable ProductConversation-to-WorkScope relation immediately** —
   normalizes attachment early, but duplicates the still-authoritative legacy
   scope fields before all attachment readers and writers can move together.
5. **Use a separate Coordinator table** — makes lifecycle absence obvious, but
   duplicates aggregate identity, transcript membership, and runtime relations
   for a ProductConversation kind that shares those semantics.
6. **Store mutable root and latest transcript identifiers on ProductConversation**
   — makes some reads cheaper, but duplicates continuation topology and creates
   another authority that can drift.
7. **Persist ProductConversation now and cut each authority over atomically at its
   owning boundary** — establishes aggregate identity for Close without allowing
   dormant lifecycle or attachment projections to become premature authorities.

## Decision

Phoenix persists **ProductConversation** as a first-class aggregate. The aggregate
owns its independently allocated identity and a typed kind: `ordinary` or
`coordinator`. Ordinary aggregates have Open/History lifecycle. Coordinator uses
the same aggregate relation for identity and transcript membership, but its
schema and domain shape make ordinary lifecycle inapplicable rather than merely
nullable by convention.

Every durable `Conversation` transcript/execution row explicitly references one
ProductConversation. Continuation topology remains owned by transcript rows.
Parent rows form the user-visible transcript topology; subordinate execution rows
are participants rather than topology members. Every continuation edge is
parent-only, same-aggregate, linear, and acyclic. Root and latest transcript
identities are derived from that topology and are not stored as mutable aggregate
columns.

ProductConversation owns stable aggregate identity, membership and topology,
canonical navigation, ordinary lifecycle, and aggregate presentation. Transcript
members retain message persistence, SSE publication, runtime and provider
sessions, and generation-fenced prompt projection. Phoenix does not add an
aggregate-native message store, SSE stream, runtime/provider session, or prompt
projection that would duplicate those member authorities.

Phoenix continues using the current Project-backed repository model. Replacement
is deferred until a named feature requires a different repository authority.

`CloseObligation` references ProductConversation. Its immutable member snapshot
references the ordered parent transcript-row continuation topology that belonged
to that aggregate when the attempt was admitted; subordinate execution rows remain
aggregate participants rather than continuation-topology members. The snapshot has
one normalized representation: exact attempt-bound parent rows with contiguous
continuation ordinals. Admission creates those member rows atomically with the
obligation and blocks a continuation successor while that attempt remains
non-completed. Completed obligations retain a typed `archived` or `cancelled`
outcome. Inspection,
inventory, loss, and retirement-evidence records remain
normalized and exact-attempt-bound; they are re-anchored to this aggregate-owned
Close attempt rather than discarded.

Legacy backfill may seed `product_conversations.id` from the continuation-root
transcript identifier. This is a deterministic migration choice, not identity
aliasing: the values occupy distinct identity and foreign-key domains. Runtime
creation allocates ProductConversation identity independently and must never
infer it from a root transcript identifier.
Compatibility carriers identify their domain explicitly: a root-oriented field
continues to carry a transcript-row identity, while an aggregate field carries a
ProductConversation identity. Raw byte equality never authorizes substitution.

Authority moves in stages, with one writable authority for each fact:

- The dormant foundation may backfill an ordinary lifecycle projection, but no
  reader or writer uses it. There are no lifecycle dual writes.
- The dormant foundation creates no runtime-visible non-completed Close obligation;
  live attempt admission begins only with the Close orchestration cutover.
- The lifecycle cutover recomputes ProductConversation lifecycle from legacy
  truth inside the same transaction that moves all lifecycle readers and writers.
  It does not trust the dormant value as an incrementally synchronized source.
  After that cutover, legacy `archived` state is derived compatibility output.
- ProductConversation-to-WorkScope attachment remains a derived projection of
  the existing authoritative representation. No new writable attachment relation
  is introduced by the dormant foundation. A normalized attachment relation may
  become authoritative only in a coordinated cutover that moves every relevant
  reader and writer together; it is never dual-written as a peer authority.

The delivery boundaries separate the dormant aggregate-and-evidence foundation
from lifecycle authority cutover and from live Close/resource-retirement
orchestration. Compatibility fields remain authoritative until the cutover that
owns their consumers; foundation persistence does not activate later behavior.

## Consequences

- **Positive:** Close obligations and evidence have the same durable owner as the
  user-facing aggregate without making a transcript segment masquerade as that
  aggregate.
- **Positive:** Invalid Coordinator lifecycle and cross-aggregate transcript
  membership can be rejected structurally.
- **Positive:** Each rollout stage has one declared writable authority, so a
  partially deployed reader cannot observe an incrementally stale peer value as
  truth.
- **Positive:** Existing normalized Close evidence work remains useful and is
  re-anchored rather than rewritten as orchestration.
- **Negative:** The lifecycle cutover must recompute legacy truth atomically even
  though a dormant backfill value already exists.
- **Negative:** Attachment remains a derived query until a later coordinated
  cutover; the dormant schema cannot treat a cleaner-looking relation as writable
  authority early.
- **Neutral:** Backfilled aggregate and root transcript identifiers may have equal
  bytes, but equality carries no runtime semantic meaning.
- **Neutral:** Some compatibility APIs continue to use root-oriented names until
  their consumers move; those names do not change the persistence authority.

## References

- ADR-026, for ProductConversation lifecycle, transcript topology, and WorkScope
  resource ownership.
- `specs/bedrock/requirements.md` and `specs/bedrock/bedrock.allium`
- `specs/chains/requirements.md`
- `specs/global-recall/requirements.md`
- `specs/projects/requirements.md` and `specs/projects/projects.allium`
- `specs/work-lifecycle/requirements.md` and
  `specs/work-lifecycle/work-lifecycle.allium`
- Key symbols: `ProductConversation`, `Conversation.product_conversation`,
  `Conversation.continued_in_conv_id`, `CloseObligation`, `AttachedWorkScope`
