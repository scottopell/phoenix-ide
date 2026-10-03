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
```

Capture uses SQLite's online backup API from a read-only source connection,
retrying busy failures with a bounded deadline. It atomically creates
`captured.db`, checks integrity on the copy, records its hash/counts, and
recovers exact query text only from the named observed call id. It refuses to
invent the exact scenario when the call cannot be recovered.

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
bytes; no errors are silently dropped. `compare` refuses mismatched fixture or
scenario digests.

Set `PHOENIX_SEARCH_BENCH_EXPLAIN=1` for `EXPLAIN QUERY PLAN` captured outside
timed runs, using the exact request builder and binds (including the generated
lexical expression). Results include the fixture/scenario digests, source
commit, host/platform/CPU, pool and read-only PRAGMAs, result counts/identity,
and private raw output. Per-case warmup failures are retained and fail the run;
case invocations have a bounded timeout and are stopped rather than stacked.
The comparison command refuses missing metadata, fixture/scenario/regime
mismatches, or changed output digests and prints paired per-case warm medians.
This harness does not optimize retrieval, rebuild FTS, migrate, ANALYZE,
checkpoint, or claim that an idle snapshot reproduces production contention.

## Initial execution status

The first production capture attempt was refused by the free-space guard:
2.9 GiB available versus a roughly 4.8 GB source database. No production
snapshot or measured baseline exists yet. Allow at least twice the source
size for capture plus headroom for release compilation, or use another
private artifact volume. Do not delete unrelated data or bypass the guard.

Two production calls are preconfigured (19.058 s and 11.890 s tool durations).
Scenario preparation requires both exact query inputs to be recovered and
refuses to substitute a duplicate. The selective case is a candidate term
from the observed query, not yet proven selective; verify its result count
and match breadth when the snapshot is available before treating it as a
selectivity contrast. Captured FTS freshness is assumed for read-only runs;
no reconciliation is performed or freshness guarantee fabricated.
