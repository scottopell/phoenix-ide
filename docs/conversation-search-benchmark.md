# Production conversation-search benchmark

This is an opt-in, local measurement rig. It never starts Phoenix against a
fixture and never writes to the production database. All artifacts contain
conversation data and are private; `conversation-search-benchmark/` is ignored.

## Capture and freeze

```sh
./dev.py conversation-search snapshot \
  --source "/private/operator-snapshots/consistent-standalone.db" --offline-snapshot \
  --artifacts conversation-search-benchmark
./dev.py conversation-search prepare --artifacts conversation-search-benchmark
# Replacing frozen scenarios is destructive and requires an explicit override:
# Scenarios are immutable after preparation; use a new dedicated directory for a new suite.
```

First obtain a consistent standalone OFFLINE snapshot by an operator-supported
process; never raw-copy a live SQLite file. Ingestion uses SQLite's backup API
from the explicitly attested immutable offline source,
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
refuses to substitute a duplicate. Older fe2 runs lacked full freshness and
selectivity qualification. Actual063a historical pair validated fingerprints;
current source validates eligible representative cases but has not been remeasured.
Replacement call recovery requires an exact transcript in
`PHOENIX_SEARCH_REPLACEMENT_TRANSCRIPT`, avoiding full-corpus content scans.

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
separately. Older fe2 runs only checked index row counts. Actual063a paired qualification
performed the full read-only content-fingerprint and physical-mapping sweep;
current source still validates freshness without rebuilding the captured index.

## Corrected historical qualification and current source boundary

The fe2f6a506 table above is historical and superseded for experiment pairing.
The actual qualified before suites ran source063a82ff2: exact tool medians
1668.460/1684.375ms; scoped retriever4360.658/4354.053ms. The paired experiment
ran source42740b6c1: scoped73.722/72.906ms, all complete result digests equal
across11surfaces. Both measured source versions validate full captured index
fingerprints and physical mappings read-only. Raw files are
`qualified-before-1/2.json` and `after-scope-1/2.json`; prior raw evidence is
retained, not rewritten. These are historical measured commits, NOT a claim
that later committed harness heads were measured.

The current harness avoids parsing unsanitized snippets. It precomputes one
structured service result per tool case before the sample sequence, then
checks timed tool outputs against that immutable formatter result and stores
structured count/order. No oracle query runs between samples. This setup
regime is explicit and **unmeasured**; do not compare new-regime runs to the
historical pair as though cache preparation were unchanged. No new release
campaign was performed merely for metadata or documentation corrections.

A tiny-scope regression allegation was checked in memory using the existing
Rust-linked SQLite3.51.3 archive (Python here links3.53.0).300k common-token
FTS rows and15selected rows, production-shaped locator/index plus source and
conversation joins, hidden predicate, BM25/order/LIMIT, snippet, and correlated
count: locator-first0:=M1 roughly77ms versus FTS-first0:M1 roughly24ms; ANALYZE
roughly78ms versus24ms. The reported0.23→30ms regression was not reproduced.
This dismisses that specific unsupported reproducer claim, not every possible
workload regression. Schema/provenance constants and corpus content are
synthetic; no production data or indices were mutated. Residual global
performance variation and lack of live19s reproduction remain as above.

Current first-sample label explicitly excludes eager pool/connection setup; it is
a first retrieval after pool setup with uncontrolled OS cache, not cold-start latency.
Representative broad/selective candidates must have eligible hits under the actual
service request before their sequences. No additional measurement campaign performed.

Freshness validation uses the existing production `Fts5Retriever::is_fresh_for`
source-extraction fingerprints and physical-row presence, plus orphan checks.
It does not independently hash physical FTS text or certify arbitrary cache
corruption. REQ-RET-008 owns index-cache reconciliation; this benchmark neither
changes that production authority nor repairs the private captured fixture.
A physical-text/sourcehash mismatch remains a separate DB invariant question,
not a new benchmark reconciliation engine.

Interrupted capture policy is fail-closed: preserve incomplete prior files and
choose a NEW dedicated artifact directory. No automatic capture recovery,
partial-file deletion, replacement or compatibility guarantee is provided.
A failure removing the completed pending marker does not delete a published
verified fixture/manifest.

Snapshot ingestion requires an explicitly supplied, operator-attested consistent
OFFLINE standalone database, not a live production file or raw live copy.
The known production path and any WAL/SHM/journal sidecars are refused before
SQLite open. Operator provenance/consistency is a precondition, not inferred
from mode=ro or absent sidecars. The existing private verified fixture and
historical measurements remain valid evidence. The original live-online-capture
acceptance is withdrawn: standard read-only online backup may alter SHM
coordination bytes, which is not evidence application data changed. No custom
VFS, permission changes, daemon, or new baseline campaign is introduced.

Published measured-source refs (source only; no private artifacts):
- Before: https://github.com/scottopell/phoenix-ide/commit/063a82ff25d9fa31ecc6d48b628196603a7a7638 (`evidence/scoped-search-measured-before`).
- After: https://github.com/scottopell/phoenix-ide/commit/42740b6c1d84ceed48ad71e36553d6f996480e3c (`evidence/scoped-search-measured-after`).
The measured after diff contains only the same unary-plus scoped predicate
and focused correctness test as independent PR838 (serialization test
representation differs on main); measurement source is durably inspectable.

Failed/interrupted suites are never accepted as measurements. Per-invocation
failures retain raw evidence when the harness returns it; an outer termination
may retain only a failure record, not completed in-memory samples. The rig
does not promise resumable sample checkpointing or partial-suite recovery.
Snapshot ingestion requires a NEW EMPTY directory; even lock-only roots are
refused before permission changes. Existing recognized fixtures remain readable
by prepare/run, not replaceable by snapshot ingestion.

Supported compilation is the pinned bundled SQLite build only. Alternate
pkg-config/library override builds are refused, not assigned a general library
identity mechanism. One artifact-wide active-run reservation prevents accidental
second commands; it is not a scheduler or a guarantee against unrelated host
load. Single-operator/no competing host load remains a measurement precondition.
Locator metadata corruption, like physical-text corruption, remains the separate
production freshness-authority assessment59007; this rig uses that authority,
not a shadow reconciliation/provenance engine. Historical returned provenance
was independently checked from source rows for the paired fixture.

Run reservations fail closed after uncatchable termination; marker records
owner PID for operator inspection. Automatic stale-owner reclamation, reboot
recovery and process-identity authentication are not supported. Confirm no
active benchmark process before removing a stale reservation manually; no
samples from an interrupted suite are accepted.

Build support is deliberately constrained to a plain host bundled release build.
Custom Cargo runners/linkers/incremental overrides are unsupported. This is
not an exhaustive hermetic toolchain identity guarantee: use a controlled
host/configuration and compare recorded effective inputs, not arbitrary wrappers.
