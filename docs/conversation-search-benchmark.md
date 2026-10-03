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
raw samples. Each case has one process/pool-first-use sample followed by ten
serial warm samples. Results include success/error state, result digest and
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
