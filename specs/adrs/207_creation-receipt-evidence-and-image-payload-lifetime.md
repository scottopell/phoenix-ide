# ADR-207: Retain Creation Evidence and Replay Identity, Retire Delivery Images at Product Deletion

- **Status:** Accepted
- **Date:** 2026-10-30
- **Affects:** Creation receipts; `specs/conversation-creation/requirements.md` REQ-CCR-013 through REQ-CCR-015

## Context

Retaining public creation jobs after target deletion preserves global request identity and truthful historical publication. Two consequences required explicit ownership: unwatched initial objectives lost terminal evidence when their execution occurrence was consumed, and retained jobs kept large creation-request image delivery copies after all product transcripts were deleted.

The user selected: “Preserve historical creation status and retry identity. Purge creation-request image payloads after the product’s last transcript is hard-deleted. No immediate production cleanup is authorized.” This is a source behavior decision, not merge, deployment, or production maintenance permission.

## Decision

1. Persist an operation-owned terminal receipt at actual normal or captured direct-turn settlement, in the transaction that consumes the exact initial objective occurrence. The request's immutable publication binding establishes the original transcript/message identity. Store the first completed, failed, or cancelled outcome, including an actual failure reason; do not infer outcomes from latest transcript state or synthesize outcomes for already-erased evidence. Watch notification is not the receipt owner.
2. Separate ordered replay identity from delivery payload bytes. Retain ordinal, media type, and SHA-256 of the exact accepted UTF-8 image data string. Keep live delivery bytes unchanged while any transcript of the product survives, including when the original transcript was deleted but a continuation survives.
3. At last-transcript/product deletion, transactionally retire only creation-request image delivery copies. Keep public creation metadata, first provenance, global request identity and original publication identity. Typed retired payloads cannot be delivered or confused with an originally empty image list. Missing or partial live payloads are corruption, not retirement.
4. Apply forward migrations that derive existing image replay identities before retiring orphan product payloads in the same migration transaction. Preserve registered sparse migration membership, existing migrations, and Goal-owned migration 145. There is no reverse migration, historical-binary compatibility promise, fabricated terminal backfill, or independent production cleanup operation.

## Options considered

- **Restore deletion cascades for jobs:** rejected; destroys retained global request identity and historical publication.
- **Infer completion from publication/latest transcript state or rely on watches:** rejected; neither proves this operation's terminal outcome.
- **Retire images on original-transcript deletion:** rejected; the selected lifetime is the product's last transcript, not the original alone.
- **Keep delivery bytes indefinitely or use a periodic retention task:** rejected; bounded retry fingerprints are sufficient after product deletion, and the owning deletion transaction provides the exact lifetime boundary.

## Consequences

Identical retries remain read-only receipt reconciliation after payload retirement; changed image count, order, media type or data conflicts without repeated work. Historical publication never acquires a successor route. Replay fingerprints and original metadata remain retained, but independent transcript attachments are outside this decision. Atomic upgrade and deletion rollback, live corruption, unwatched normal/direct-turn outcomes, restart, and image-bearing historical retries require regression evidence. Source implementation alone is not qualification or permission to mutate production.
