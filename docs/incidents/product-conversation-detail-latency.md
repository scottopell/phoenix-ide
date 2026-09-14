# ProductConversation detail-read latency

## Incident evidence

After deploying `ce66022db`, successful `GET /api/product-conversations/:reference`
requests from 12:12–12:51Z had 100 samples: median 3,735 ms, p95 7,603 ms,
and maximum 8,453 ms. The immutable source files are
`/tmp/phoenix-open-perf.ikefq5/http-samples.json` and `errors.json`; their
SHA-256 digests were recorded before diagnosis. No production load test,
restart, or deployment was performed.

The same capture contains 147 detail 404 responses, all for a coordinator
reference, with median 2 ms, p95 3 ms, and maximum 23 ms. Those fast failures
are route/fallback churn, not the mechanism for slow successful detail reads.

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
messages each and records 11 warm stage samples in microseconds:

```text
resolve=[203,63,60,72,65,95,61,95,69,60,68]
aggregate=[14969,14150,14190,14006,13773,14216,14365,14279,13719,13920,13833]
page=[30665,30347,30139,30325,30331,30176,30505,30595,30171,29684,30068]
rollback=[22,33,40,49,42,45,35,62,42,33,38]
```

Warm medians are 0.068 ms resolve, 14.150 ms aggregate, 30.325 ms page, and
44.452 ms total; warm p95/max values are 0.203, 14.969, 30.665, and 45.859 ms.
The page query is the dominant reproducible stage. The production multi-second
scale is consistent with the same globally scanning plan under a 3.8 GiB,
concurrently used database; browser/network and continuation-summary changes do
not explain server-measured successful GET duration.

## Bounded fix plan

1. Force the bounded page query to drive from the recursive transcript rows and
   probe the existing message index, retaining the same predicates, ordering,
   segment ceilings, and `LIMIT` inside the same read transaction.
2. Lock the desired query plan with a representative local fixture and compare
   identical raw cold/warm samples before and after.
3. Reassess aggregate hydration only after the page fix; batch per-segment
   metadata/handoffs only if it remains a measured dominant stage.
4. Trace coordinator 404 request ownership separately. Do not mix route-churn
   work into the successful-detail fix unless a requester is proven and the
   lookup is inapplicable by type/state.

## Remaining end-to-end stages

The supplied evidence measures server HTTP duration, and the local fixture
separates SQLite stages. It does not contain response-byte, serialization,
SSE/store, or browser first-paint marks. Those stages remain unclaimed rather
than being inferred from unrelated readiness work. Privacy-safe `open.id` plus
durable product reference are the intended correlation keys for future traces;
content and secrets are excluded.
