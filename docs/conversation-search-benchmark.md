# Production conversation-search benchmark

This is an opt-in, local measurement rig. It never starts Phoenix against a
fixture and never writes to the production database. All artifacts contain
conversation data and are private; `conversation-search-benchmark/` is ignored.

## Capture and freeze

```sh
./dev.py conversation-search snapshot \
  --source "$HOME/.phoenix-ide/prod.db" \
  --artifacts conversation-search-benchmark
./dev.py conversation-search prepare --artifacts conversation-search-benchmark
# Replacing frozen scenarios is destructive and requires an explicit override:
# ./dev.py conversation-search prepare --artifacts conversation-search-benchmark --force
```

Capture uses SQLite's online backup API from a read-only source connection,
retrying busy failures with a bounded deadline. It atomically creates
`captured.db`, checks integrity on the copy, records its hash/counts, and
recovers exact query text only from the two named observed calls. For the
known transcripts (`9bc3b72d…` and
`ca640950-4b82-4499-a1af-9846827f29dc`), recovery first selects at most 2,000
messages by indexed `conversation_id`; it does not perform a multi-gigabyte
content `LIKE` scan. Synthetic fixtures without those transcript ids retain a
bounded call-id fallback. Serialized JSON tool arguments are decoded before
extracting `query`, and sibling tool blocks cannot contribute. It refuses to
invent the exact scenario when either call cannot be recovered.

## Run, report, compare

```sh
./dev.py conversation-search run --artifacts conversation-search-benchmark --label suite-1
./dev.py conversation-search run --artifacts conversation-search-benchmark --label suite-2
./dev.py conversation-search report --artifacts conversation-search-benchmark
./dev.py conversation-search compare \
  conversation-search-benchmark/runs/suite-1.json \
  conversation-search-benchmark/runs/suite-2.json
```

The ignored release-mode Rust test opens the fixture read-only through
`Database::open_read_only`, calls the real `search_conversations` Tool::run
for tool cases and `Fts5Retriever::retrieve` for the scoped case, and emits
raw samples. Each case/surface opens a fresh pool for one first-use sample,
then a discarded warmup and ten serial warm samples. The process and OS cache
are not cold: fixture hashing and preceding cases can warm the filesystem. Results include success/error state, result digest and
bytes; no errors are silently dropped. Reports group by run label/file, case,
surface, and phase, and exclude successful discarded warmups from summaries.
`compare` refuses mismatched fixture or scenario digests.

Set `PHOENIX_SEARCH_BENCH_EXPLAIN=1` for `EXPLAIN QUERY PLAN` captured outside
timed runs, using the exact request builder and binds (including the generated
lexical expression). Results include the fixture/scenario digests, source
commit, host/platform/CPU, pool and read-only PRAGMAs, result counts/identity,
and private raw output. Per-case warmup failures are retained and fail the run;
case invocations have a bounded timeout and are stopped rather than stacked.
The comparison command refuses missing or empty metadata, fixture/scenario,
environment, SQLite, runtime, or EXPLAIN-regime mismatches, changed output
digests, errors/timeouts, or incomplete per-surface ten-iteration warm suites.
It prints paired per-case/per-surface warm medians. The Python outer timeout
starts the cargo process in its own POSIX process group, terminates the group
(and escalates if needed), waits for it, and writes a private failure record
outside the raw run report.
This harness does not optimize retrieval, rebuild FTS, migrate, ANALYZE,
checkpoint, or claim that an idle snapshot reproduces production contention.

## Initial execution status

The initial capture was refused by the free-space guard. After coordinated
reclamation of owner-confirmed inactive build caches, capture and two release
suites completed locally. Allow at least twice the source size for capture
plus headroom for release compilation. Do not delete unrelated data or bypass
the guard. See the baseline summary below.

Two production calls are preconfigured (19.058 s and 11.890 s tool durations).
Scenario preparation requires both exact query inputs to be recovered and
refuses to substitute a duplicate. The selective case is a candidate term
from the observed query, not yet proven selective; verify its result count
and match breadth when the snapshot is available before treating it as a
selectivity contrast. Captured FTS freshness is assumed for read-only runs;
no reconciliation is performed or freshness guarantee fabricated.

## Baseline: 2026-10-03

Fixture: 4,766 conversations; 555,944 messages, locator rows, and physical FTS
rows. Integrity check passed. Capture hash:
`8f8fbe5a44cbf165e936cb6dc87785ba213505c89bfd4f6dd55c160d92c45d7f`.
Frozen scenario hash:
`de55a58f0c5e45ff73bd42cfd3be1127d44a942c959c4fa2f8656ce4d64880d3`.
Measured source commit: `fe2f6a506`. Both suites passed, with identical output
digests within/across runs and ten warm samples per case/surface per suite.
First-use, warmup, and warm samples are separate; first-use is fresh-pool,
not process- or physical-cache-cold. Release compilation is excluded.

| Case | Tool median ms, suite 1 / 2 | Retriever median ms, suite 1 / 2 |
|---|---:|---:|
| Observed slow query A | 1680.23 / 1675.02 | 1671.42 / 1656.28 |
| Observed slow query B | 583.77 / 585.85 | 582.21 / 576.11 |
| Broad common term | 1086.50 / 1080.28 | 1056.81 / 1058.10 |
| Selective identifier | 3.67 / 3.50 | 1.45 / 1.48 |
| No hit | 0.25 / 0.29 | 0.27 / 0.33 |
| Existing transcript scope | — | 4463.59 / 4349.68 |

Private server-local artifacts are under `conversation-search-benchmark/`:
`capture-manifest.json`, `scenarios.json`, `runs/suite-1.json`,
`runs/suite-2.json`, `report.md`, `diagnostics.json`, and `verification.json`.
They must not be committed or uploaded. The diagnostic run used the same
release test executable directly to avoid a forced Git-SHA build-script
recompile; its timings are deliberately excluded from the two baseline suites.

The two observed production tool durations were 19,058 and 11,890 ms. This
idle, warmed snapshot reproduces seconds-scale search cost, **not the full
live production latency**. Host contention, connection/cache state, and exact
deployed-build parity are not controlled. No speedup is claimed.

The literal broad/ selective/no-hit terms match 143,795 / 39 / 0 FTS rows.
The effective natural-language expressions for queries A/B match 223,979 /
64,478 physical rows before eligibility. The scoped transcript has 816
messages, yet is slower: its exact query plan starts from the conversation
locator index and invokes FTS with both rowid and MATCH (`0:=M1`), unlike the
global FTS-first plan (`0:M1`). All plans include a correlated indexed message
count and a temporary ordering B-tree. Query plans identify candidates, not
proved per-operator costs. Returned identities and hidden-message predicates
were independently verified from the snapshot for the first warm outputs of
both suites; all remaining output digests agree.

### Recommended next experiment (not implemented)

First test a scope-query-plan change that permits FTS-first matching for the
scoped case, preserving scope filtering before LIMIT and all provenance.
The 816-message scoped search taking ~4.4 s while the global equivalent takes
~1.7 s makes this the strongest bounded plan-level anomaly. Require exact
result equivalence and paired runs on this same fixture/scenario suite.
Second investigate deferring the correlated count and snippet/provenance
projection until after ranking/limiting where semantics permit. Third,
correlate live production cache/queue/CPU/IO evidence with the idle baseline
before attributing the 19 s report to any one SQL operator. Changing OR query
semantics, indexing content, or adding embeddings is not justified by these
measurements.

Validation: 12 synthetic Python tests, release benchmark tests (two suites
and diagnostic run), Rust test-target compilation, formatting, and task
validation pass. Full workspace tests/clippy are not claimed. SQLite fields
in raw runs describe configured read-only pool policy, not a complete live
PRAGMA measurement; `verification.json` labels offline Python PRAGMA reads
separately. Captured index row counts agree, but a full content-fingerprint
freshness sweep was not performed and the read-only harness deliberately does
not rebuild the captured index.
