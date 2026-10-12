# Isolated late-hydration query experiment

## Scope and identity

This is a fixture-only statement experiment, not a production query rewrite, end-to-end performance result, or historical-production causality claim. Production SQL and the existing browser replay binary are unchanged. A `cfg(test)` probe constructs an ordered `selected AS MATERIALIZED` CTE with the original topology, ceiling, cursor and lookahead predicates, then joins full message rows by globally unique ID in the same statement/transaction. The test build compares every parsed `(ordinal, Message)` field against control before passing candidate rows to the unchanged attachment hydrator. These dirty test-build timings are deliberately contaminated by executing both queries and must not be benchmarked.

Base: `d63f657947b8e4e5c800107288acbc3a263165ad`. Immutable fixture hash: `13f53c154a9af5d3e94e13fd4feb1c54af7d75d370c54faaed225ab3e7125308`. Statement harness uses the retained SQLite3.51.3 archive. Original SQL hash `68a92f6fd8ef7174dedce3127e4eb422e30180f4673f923daeaccc515e136a79`; candidate hash `1c9f7db639fcc36e195c6a679cc9b87d4869f2f5d4a9926f18187b4a0b129528`. Raw SQL/scripts/results/failures are private under `.phoenix/replay-proof/late-hydration-*`.

## Correctness evidence and limitations

Independent bounded read-only review found no SQL semantic defect. It verified unchanged recursive traversal, indexed join, ceiling/cursor inequalities, descending ordinal/sequence/message-ID total order, bound limit and complete payload/origin projection. It identified deadline, artifact and i64-bind weaknesses repaired before measurement. A later test-only change passes equal candidate rows to downstream hydration; production build excludes both the candidate and differential probe.

Before the paired round: 202 synthetic full-row cases passed for large i64 sequences, cross-segment tied sequences, each cursor boundary, combined multisegment/omitted/empty ceilings, lookahead limits and empty targets. Seven immutable-fixture cases passed (latest limits2/101, cursor limits2/101, ceiling, empty ceiling and limit0), comparing all raw returned columns/order. Focused Rust page suite14 passed/1 ignored; product-specific suite22 passed/1 ignored; attachment suite21 passed. Existing source-call provenance, cancellation/clean connection, lifecycle/reference rejection and snapshot/attachment transaction tests are retained. A zero-test filter is explicitly not counted.

**Historical gate gap at measurement time:** the paired round was performed before direct owning-boundary generation/reset and compositional alignment/topology coverage were completed. That did not follow the explicit correctness-before-timing sequence. The pairs remain exploratory and are not retroactively accepted after subsequent tests. The correctness follow-up below closes the bounded isolated-component gate; actual candidate compilation/execution through the API dependency remains a separate promotion integration gate. No end-to-end improvement is claimed.

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

## Final bounded correctness disposition

No further timing, production reads, native fixture server or capture occurred. Two new tests remain confined to the phoenix-db unit-test module; the candidate and differential seam remain cfg(test). A file-backed disposable WAL test establishes an actual reader snapshot through aggregate metadata, commits message deletion plus generation advancement through a second connection, then executes the differential candidate page and shared attachment hydration inside the reader transaction. It checks complete rich user content (expanded text/meta flag, image/file attachments), display/usage, source-call origin and timestamp against pre-reset normal message hydration, plus unchanged root conversation metadata/generation/tail. A fresh public snapshot must be empty with the new generation. This is a causally forced committed reset interleaving, not scheduling luck or a sleep-based race oracle.

The second test exercises the real domain snapshot helper: initial descending order/lookahead, append, exclusive cursor, generation/request identity and empty ceilings. Independent review found its original nonempty ceiling could be masked by its cursor. A separate no-cursor ceiling oracle now requires the bounded page to start at m4/exclude newly appended m5 while the unbounded page starts at m5. Existing synthetic202 full-row cases retain cross-segment ties, large i64 sequences, multisegment cursor/ceilings and empty variants.

Library-level observation/product tests cover actual disconnected target/namespace admission, malformed/excluded references, render-aligned tail at current generation, cursor scope/staleness and full metadata projection. Direct handler tests cover deterministic generation-change and same-generation alignment retry. **Compilation scope matters:** these API tests depend on normal phoenix-db and therefore use control SQL, not its unit-test candidate. The bounded compositional argument is candidate/control full typed-row equality inside the transaction plus the identical downstream hydrator/projection/admission/retry path. It is not a claim of candidate-specific end-to-end API execution or proof of arbitrary concurrent schedules.

Independent frozen-diff review found no blocking correctness defect and accepted that bounded compositional gate subject to completed parent test evidence. It verified primary-key hydration cannot fan out, original predicates/order/limits are retained, WAL deletion cascades exercise old attachment visibility, candidate rows actually feed hydration, and no production query change. The independent review covered HEAD `2dc9d827b3eb1611efbfbc224830078492f49f4b` plus frozen dirty-diff SHA-256 `c2ff7ede3b70dcc36d9f5fb56e5ddc8190b16a9a7725bf6277b2c0b5fa920bd1`. Parent inspection subsequently covered the non-vacuous ceiling oracle, full root-conversation metadata equality and helper extraction; these final test-only changes were not independently re-reviewed. This is not a cumulative exact-final-HEAD review claim. The next promotion scope is narrowly to compile the real candidate into the API dependency in an isolated proposed implementation, run these same owning tests at that head and review its query-plan/build behavior; do not promote merely by flipping the experimental test probe. Shared UI/LiveState/collector/ADR and migration/deployment holds remain Global-owned and unchanged.

Test-fixture errors were not implementation findings: initial file Database::open omitted disposable schema migrations, and cleanup held the reader connection while awaiting pool close. Both were repaired; failed/stopped logs retained. The corrected DB page module passed16 tests (1 ignored), API observation8 and product28 passed. On the final test source, the serialized command chain exited0 with `CARGO_BUILD_JOBS=1` and `--test-threads=1`:

| Command/filter | Actual receipt |
|---|---|
| `cargo test -p phoenix-db --lib late_hydration_snapshot` | 2 passed; 0 failed/ignored |
| `cargo clippy -p phoenix-db --all-targets -- -D warnings` | exit0 |
| `cargo test -p phoenix_ide --lib generation_race` | 5 passed; includes full-conversation, slug, around, exact-range and stable-stream retry |
| `cargo test -p phoenix_ide --lib transcript_generation_changes_mid_read` | 2 passed; before/latest slice retry |
| `cargo test -p phoenix_ide --lib message_slice_aligns_to_agent_turn_boundary` | 2 passed; before/latest alignment preserving has_older |

`cargo fmt --all` completed and `git diff --check` passed. The earlier `get_full_conversation_retries` filter selected zero tests and is rejected as evidence. Final logs are retained privately under `.phoenix/replay-proof/late-hydration-{helpers-final,lint-repaired,retry-actual}.log`, alongside earlier passing and failed/stopped receipts. No full-repository green claim is made: previously recorded unrelated Bash-retirement and LLM Allium failures remain outside this qualification. No production behavior change or historical speedup is claimed.
