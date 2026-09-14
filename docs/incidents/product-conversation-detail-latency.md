# ProductConversation detail-read latency

## Incident evidence

After deploying `ce66022db`, successful `GET /api/product-conversations/:reference`
requests from 12:12–12:51Z had 100 samples: median 3,735 ms, p95 7,603 ms,
and maximum 8,453 ms. The immutable source files are
`/tmp/phoenix-open-perf.ikefq5/http-samples.json` and `errors.json`; their
SHA-256 digests were recorded before diagnosis:
`a623926f389bb0850c0486f771aad0e571607a03765ff6f55196798cfbf74f0d` for
`http-samples.json` and
`39bd1be8f064ba6fb5b8a097b5398350463e342bda4042f5bcf85041961be0d6` for
`errors.json`. No production load test,
restart, or deployment was performed.

The same capture contains 147 detail 404 responses, all for a coordinator
reference, with median 2 ms, p95 3 ms, and maximum 23 ms. Those fast failures
are route/fallback churn, not the mechanism for slow successful detail reads.
The requester is `ProductConversationAliasRedirect`, mounted on legacy
`/c/:slug` and `/chains/:rootConvId`: it probes ProductConversation detail and
falls back to `EmbeddedConversationPage` after a 404. The capture does not prove
why coordinator references enter those alias routes in bursts, so this work does
not alter or coalesce that behavior.

## Read path and invariants

`Database::read_ordinary_product_conversation_snapshot` acquires one pooled
connection, starts a deferred read transaction with `BEGIN`, resolves the
reference, hydrates the recursive aggregate, reads one bounded message page and
its attachments, and rolls back. The fix must preserve that one read-only
snapshot, product-reference fencing, cursor segment ceilings, and transcript-tail
chronology. `BEGIN IMMEDIATE` is not valid for this read.

The deployed query count for an aggregate with `S` transcript segments is:

- one resolve query;
- one product row plus one recursive topology query;
- `S` calls to `get_conversation_on`;
- up to `2 * (S - 1)` handoff queries, because an absent completed handoff falls
  back to a historical boundary query;
- one source query;
- one message-page query;
- up to two batched attachment queries.

This is at most `5 + 3S` statements before attachments and 76 statements for a
23-segment aggregate. The per-segment awaits are a secondary linear cost.

## Representative scale and query plan

A read-only production inventory established the representative shape without
reading message content: the largest ordinary aggregate has 23 parent transcript
segments and 23,449 messages. `EXPLAIN QUERY PLAN` for its 51-row page showed:

```text
SCAN messages USING INDEX idx_messages_conversation
SEARCH transcript USING AUTOMATIC COVERING INDEX (id=?)
USE TEMP B-TREE FOR ORDER BY
```

SQLite therefore scans the global message index and sorts before applying the
bounded limit, instead of driving from the 23 transcript rows and probing the
`messages(conversation_id, sequence_id)` index.

The ignored deterministic diagnostic
`product_conversation_snapshot_scale_diagnostic` builds 23 segments with 1,020
messages each plus 200,000 unrelated messages, then records one cold and ten
warm stage samples. The unrelated population is required to reproduce the
planner cliff: without it both plans touch approximately the same 23k rows.

Baseline raw samples in microseconds:

```text
[(261,15341,90121,105),(76,14823,89144,38),(69,14735,89136,36),
 (69,14930,89381,40),(78,13919,88481,64),(81,14421,88544,34),
 (66,14888,89389,81),(70,14578,89114,42),(67,14440,89457,34),
 (90,14833,88784,35),(70,15221,89267,44)]
```

Each tuple is `(resolve, aggregate, page, rollback)`. The baseline cold total is
105.828 ms. Warm p50/nearest-rank p95 are 0.070/0.090 ms resolve,
14.779/15.221 ms aggregate, 89.140/89.457 ms page, and 103.987/104.602 ms
total.

After forcing transcript-first indexed probes, identical-shape raw samples are:

```text
[(243,15169,25565,29),(59,14139,25436,40),(58,14785,26043,40),
 (61,15130,25637,57),(64,14462,25474,44),(61,14968,25292,39),
 (98,14549,25152,34),(66,14768,25173,37),(62,14910,25415,35),
 (67,15788,34123,43),(232,16754,26473,45)]
```

The candidate cold total is 41.006 ms. Warm p50/nearest-rank p95 are
0.063/0.232 ms resolve, 14.848/16.754 ms aggregate, 25.455/34.123 ms page, and
40.391/50.021 ms total.
The page p50 falls 71%, and total p50 falls 61%, while aggregate hydration is
unchanged within run noise. The production multi-second scale is consistent
with the globally scanning plan under a 3.8 GiB, concurrently used database;
browser/network and continuation-summary changes do not explain server-measured
successful GET duration.

## Bounded fix plan

1. Force the bounded page query to drive from the recursive transcript rows and
   probe the current-schema `messages_conversation_sequence` index, retaining
   the same predicates, ordering, segment ceilings, and `LIMIT` inside the same
   read transaction.
2. Keep the representative diagnostic fixture and a structural assertion on
   the transcript-first indexed join as regressions for the bounded hot path.
3. Reassess aggregate hydration only after the page fix; batch per-segment
   metadata/handoffs only if it remains a measured dominant stage.
4. Trace coordinator 404 request ownership separately. Do not mix route-churn
   work into the successful-detail fix unless a requester is proven and the
   lookup is inapplicable by type/state.

## Remaining end-to-end stages

The supplied evidence measures server HTTP duration, and the local fixture
separates the transaction's SQLite stages. After that transaction,
`snapshot_view` performs close-projection, source-deletion, and writable-row
lookups before serialization; these were not separable in the supplied capture.
It also does not contain response-byte, serialization, SSE/store, or browser
first-paint marks. Those stages remain unclaimed rather than being inferred from
unrelated readiness work. Privacy-safe `open.id` plus
durable product reference are the intended correlation keys for future traces;
content and secrets are excluded.
