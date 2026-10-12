# Isolated late-hydration query experiment

## Scope and identity

This is a fixture-only statement experiment, not a production query rewrite, end-to-end performance result, or historical-production causality claim. Production SQL and the existing browser replay binary are unchanged. A `cfg(test)` probe constructs an ordered `selected AS MATERIALIZED` CTE with the original topology, ceiling, cursor and lookahead predicates, then joins full message rows by globally unique ID in the same statement/transaction. The test build compares every parsed `(ordinal, Message)` field against control before passing candidate rows to the unchanged attachment hydrator. These dirty test-build timings are deliberately contaminated by executing both queries and must not be benchmarked.

Base: `d63f657947b8e4e5c800107288acbc3a263165ad`. Immutable fixture hash: `13f53c154a9af5d3e94e13fd4feb1c54af7d75d370c54faaed225ab3e7125308`. Statement harness uses the retained SQLite3.51.3 archive. Original SQL hash `68a92f6fd8ef7174dedce3127e4eb422e30180f4673f923daeaccc515e136a79`; candidate hash `1c9f7db639fcc36e195c6a679cc9b87d4869f2f5d4a9926f18187b4a0b129528`. Raw SQL/scripts/results/failures are private under `.phoenix/replay-proof/late-hydration-*`.

## Correctness evidence and limitations

Independent bounded read-only review found no SQL semantic defect. It verified unchanged recursive traversal, indexed join, ceiling/cursor inequalities, descending ordinal/sequence/message-ID total order, bound limit and complete payload/origin projection. It identified deadline, artifact and i64-bind weaknesses repaired before measurement. A later test-only change passes equal candidate rows to downstream hydration; production build excludes both the candidate and differential probe.

Before the paired round: 202 synthetic full-row cases passed for large i64 sequences, cross-segment tied sequences, each cursor boundary, combined multisegment/omitted/empty ceilings, lookahead limits and empty targets. Seven immutable-fixture cases passed (latest limits2/101, cursor limits2/101, ceiling, empty ceiling and limit0), comparing all raw returned columns/order. Focused Rust page suite14 passed/1 ignored; product-specific suite22 passed/1 ignored; attachment suite21 passed. Existing source-call provenance, cancellation/clean connection, lifecycle/reference rejection and snapshot/attachment transaction tests are retained. A zero-test filter is explicitly not counted.

**Promotion gate still incomplete:** these passes do not directly exercise this candidate under every aggregate generation-reset race/alignment retry or every malformed/disconnected topology admission case. Those upstream paths are unchanged and the SQL remains in the same transaction, but source preservation is not direct candidate integration proof. The paired measurements below are exploratory statement evidence, not a review-qualified implementation acceptance. Do not promote the query until explicit candidate integration cases cover those lifecycle boundaries and a full review at the integration source; no end-to-end improvement is claimed.

Initial test-only binding/type compile errors were repaired. An initial correctness control hit its2s progress deadline during local build/test contention; no measurements were accepted from that run. Its failure is retained. The overly broad full DB suite was stopped by this owner to avoid extended contention; not reported green. Narrow gates then completed. SQL and hashes are saved before the final fixture gate; measurement has2s statement progress checks, post-row deadline assertions and120s external process watchdog. No replacement measurement round was launched.

## One alternating paired round

Eight pairs, four per limit, order balanced separately per limit. Same read transaction per pair; full rows equal in every pair. Total gate+round6.95s, within120s and the approved five-minute ceiling. Correctness setup warmed the connection/cache; **no cold result** is claimed. Prepare/bind/step/column-extraction elapsed is measured; finalize, transaction setup, Rust decoding, attachments and HTTP are outside this harness window. No GC/cache reset, new capture, production page execution, fixture writes or other-owner shutdown occurred.

|Pair|Requested limit|Order|Control ms|Candidate ms|
|---|---:|---|---:|---:|
|1|1|control→candidate|169.51|64.83|
|2|100|control→candidate|425.52|143.55|
|3|1|candidate→control|165.40|66.45|
|4|100|candidate→control|425.09|139.19|
|5|1|control→candidate|166.30|63.84|
|6|100|control→candidate|450.86|446.04|
|7|1|candidate→control|395.98|66.02|
|8|100|candidate→control|461.16|160.43|

Limit1 VMsteps: control1080712, candidate802047. Limit100: control1082098, candidate804225. One SQLite sort reported in both, not evidence of equal sort width/work. Medians: limit1 control167.90/candidate65.43ms; limit100 control438.19/candidate151.99ms. Pair6 candidate446.04ms is retained, not replaced. Both execution-order strata generally show the same direction, but eight pairs are not significance evidence or an accepted budget. Own build/test commands were idle during measurement; ambient host load remained~50–56 before and47–54 afterward on10 logical CPUs. Other owners remained active; no saturation percentage or precise contention attribution is inferred.

## Decision

The candidate is worth further correctness-qualified integration: it preserves complete observed rows while reducing VM work and usually statement elapsed, consistent with avoiding wide payload materialization during broad candidate selection. The experiment does not identify an exclusive sorter/table-access/CPU mechanism or explain the historical2.365945s span. No production rewrite/merge/deploy occurred. Keep the experiment separable from replay: promote only after direct generation/alignment/invalid-topology gates, then compare real shared-handler/browser behavior under an explicitly identified build. Resource/UI/LiveState/collector and ADR reconciliation remain Global-owned integration obligations; no other worktree was overwritten.
