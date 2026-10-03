# Benchmark conversation search against a production database snapshot

## Goal and boundary

Build a repeatable, local benchmark rig for `search_conversations` using a consistent snapshot of the user's current production database. Reproduce representative slow searches, establish a baseline, and identify the next bounded optimization experiment. This task delivers measurement infrastructure and evidence, not a search optimization or production deployment.

## Observed journey and verified evidence

- User reports search conversation tools taking about 17 seconds.
- Production log on 2026-10-03 records `call_w7yaFJY51rJxTjog4DKeE2wo` in transcript `9bc3b72d-70ac-46db-99e0-f842321450a7`: tool duration 19,058 ms and ranked SQL duration 19.054056 seconds. Recent other examples: 11,890 ms, 10,944 ms, and 9,384 ms. SQL accounts for nearly all elapsed time in these samples.
- `~/.phoenix-ide/prod.db` is approximately 4.8 GB and has an active WAL. Copying the main file alone is not a reliable fixture capture.
- Verified local journey: `coordinator_tools::SearchConversations::run` → `api::global_read::GlobalReadService::search` → Coordinator-chain exclusion lookup → `Fts5Retriever::retrieve` / `retrieve_match_expr` → citation formatting → tool-output serialization.
- Retrieval uses one FTS5 index plus `message_fts_rows`, messages, and conversations; ranking includes BM25, snippets, hidden-message filtering, and a correlated message-count subquery. These are investigation candidates, not proven root causes.
- Normative authority: `specs/conversation-retrieval/requirements.md`, especially REQ-RET-001 through 008; current implementation summary in its executive document. Preserve scope, eligibility, ranking, limit, freshness, and provenance semantics.
- Existing retrieval tests use small synthetic corpora; they are correctness coverage, not a production-scale baseline.
- Roadmap #806 generated body was read during Explore. No roadmap ownership or priorities are changed by this task.
- Recent production logs show trace-export HTTP 429 errors. A bounded Tempo query for slow `tool.execute` spans returned no traces; missing traces cannot establish absence of slow calls. Logs provide independent deployed evidence.

## Unknowns to resolve during execution

Exact queries for the observed slow calls; deployed build/version versus benchmark checkout; corpus and FTS size; index freshness; query selectivity; connection/cache settings; and the contribution of concurrent production activity. Recover exact query inputs through bounded reads of the named tool calls from the snapshot. Do not assume local HEAD equals the deployed build, or promise identical wall time on an idle snapshot.

## Implementation plan

### 1. Capture and identify a safe fixture

- Add an explicit snapshot command through `./dev.py`. Use SQLite's supported online backup mechanism with a read-only source connection and bounded busy/retry handling. Capture committed WAL state consistently; never checkpoint, migrate, reconcile, vacuum, or otherwise write to production. Fail clearly rather than falling back to a raw live-file copy.
- Check available disk space for the snapshot plus any working copy. Put database, manifests, scenario inputs, and raw results in a private, ignored local artifact directory, outside production paths. Do not commit or upload the database, transcripts, snippets, or private query text.
- Complete capture atomically; reject incomplete fixtures. Run integrity checks on the snapshot, not on production. Record capture time, snapshot hash/size, schema/migration identity, SQLite/FTS version, and corpus/index counts.
- Keep the captured fixture immutable. Any migrations or reconciliation needed by the benchmark checkout occur only on an explicit disposable copy, outside measurement; record the changes and distinguish captured versus prepared fixture hashes. Preserve the captured FTS index by default: no unreported rebuild, ANALYZE, or optimization before baseline.
- Prevent benchmark execution against production or the capture source. Avoid launching the whole application on a cloned DB: it contains durable conversations/workflows that must not resume or execute tools. The harness opens only the services needed for read-only search.

### 2. Exercise real search code

- Implement a small release-mode Rust harness using the existing retriever and real `GlobalReadService::search` / tool adapter. Prefer an ignored benchmark test or focused harness within the owning crate over broad visibility changes or a new public application API.
- Time the complete `Tool::run` search invocation, without LLM generation, and the retriever independently with the same resolved request policy. Separate fixture/setup/build/reconciliation time from query time. Where practical, expose host exclusion lookup and citation/serialization costs without introducing a second implementation of search.
- Match and report relevant production SQLite/sqlx pool configuration, PRAGMAs, worker/runtime settings, compilation profile, and connection counts. Reuse production query construction and bind values; no hand-maintained SQL benchmark replica.
- Provide an opt-in diagnostic mode capturing EXPLAIN QUERY PLAN for the exact generated statement and binds, outside timed runs. Keep any instrumentation narrowly scoped and behavior-neutral.

### 3. Freeze a small representative scenario suite

Use about six cases rather than a combinatorial suite:

1. The exact recent approximately 19-second production tool query.
2. One other observed slow natural-language query with a different term mix.
3. A common/high-frequency term query stressing broad matches.
4. A selective identifier or uncommon term with known matches.
5. A verified no-hit query, exercising fast-path/result handling.
6. A restricted existing chain/transcript scope using the shared retrieval primitive, to contrast global corpus cost with scope cost.

Use the tool's actual global exclusions and result limit for primary tool cases. Record original query, effective lexical expression, scope, visibility/grouping/match policy, limit, and expected results locally. Select all cases once during setup; never randomly sample or retune cases between comparison runs. Palette prefix/grouping and concurrency/load testing are optional follow-ups, not prerequisites for this tool-focused baseline.

### 4. Collect reliable measurements and correctness evidence

- Record one first-use sample from a fresh process/pool separately. Label it process/connection-cold, not OS-cache-cold; do not purge system caches or perturb production.
- Use explicit discarded warmups and at least 10 measured serial warm runs per case, with deterministic documented case ordering. Repeat the suite in a second fresh process to assess stability. Use monotonic clocks.
- Retain every raw duration, success/error, result count, result identity/order, and result digest; errors/timeouts must never be silently dropped or counted as successful fast searches. Choose and record a generous per-case timeout exceeding observed latency; stop rather than stacking work after timeout.
- Summarize median, min/max, and spread (e.g. IQR); document percentile convention and avoid unsupported tail/SLO claims from ten samples. Record machine/OS, commit, build profile, run time, fixture hash, PRAGMAs/pool settings, and cache/warmup regime.
- Capture local baseline result expectations: returned message/conversation IDs, ordering/scores, snippets, provenance, and serialized tool response as appropriate. Verify repeatability and scope/limit/hidden-message rules. If genuine score/time ties exist, record them explicitly rather than adding a production tie-breaker as part of this benchmark task.
- Persist machine-readable raw samples plus a readable baseline report. Include an A/B comparison command that refuses mismatched fixture, scenarios, or measurement regime; no optimization or speedup claim without paired comparable evidence.

### 5. Validate and hand off the optimization plan

- Add small synthetic tests for snapshot consistency (including committed WAL writes), incomplete-fixture failure, production-target refusal, report parsing, error handling, and scenario/result checks. These tests run without the private prod fixture. Full-scale benchmarks remain explicit opt-in, not a normal CI dependency or timing gate.
- Document repeatable `./dev.py` capture, baseline, rerun, diagnostics, and comparison commands. Run relevant repository validation through `./dev.py check`.
- Deliver baseline evidence and query plans. Explain whether the observed slow calls reproduced, their range, and differences from deployed evidence. A failure to reproduce is reported honestly with a next diagnostic step, not papered over by selecting a slower replacement query.
- Rank a small number of follow-up optimization hypotheses by evidence and expected semantic risk (e.g. ranking/match breadth, per-candidate counts/snippets/joins, scope query plan). Recommend one bounded first experiment that will use this same rig, preserve retrieval contracts, and require before/after correctness and performance evidence.

## Acceptance criteria

- A consistent, private full-production snapshot can be captured while production remains running, without modifying the live DB or activating cloned workflows.
- One documented command benchmarks actual search code against a fixed identified fixture in release mode; a later rerun reuses that fixture rather than silently recapturing production.
- Representative cases include exact observed slow queries, broad/selective/no-hit behavior, and a scoped comparison.
- Baseline contains raw repeated samples, separate first-use/warm results, environment identity, output checks, and exact-query diagnostic plans.
- At least two suite executions support the repeatability assessment; unresolved variance and reproduction differences are disclosed.
- No search behavior, ranking algorithm, schema/index optimization, production service, or production data is changed in this task.
- Small harness tests pass without access to the user's database. Private fixtures/results do not enter Git or public artifacts.

## Risks and non-goals

The fixture contains sensitive history and occupies several GB; backups and hashing can create IO load. Use bounded capture and document the impact. An idle offline fixture measures search capacity, not live contention. The first-use run cannot prove physical-disk-cold behavior. Fixture preparation can accidentally erase the issue, so changes must be explicit and separately identified. Collector warnings are recorded as an evidence limitation, not repaired here. No embedding/vector backend, UI work, general-purpose performance framework, broad tracing project, production deploy/restart, or latency threshold commitment is included.
