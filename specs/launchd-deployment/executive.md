# Native macOS launchd Deployment — Executive Status

## Current reality

The protocol-3 deploy helper implements ordinary local/published deployment. It
validates exact candidate artifacts and checks backup capacity for an existing
database before disruption; it quarantines the canonical plist and observes
predecessor stop. For an existing database it holds a bounded SQLite
`BEGIN IMMEDIATE` writer reservation across backup verification and durable
boundary recording, retaining the fsynced native backup and transaction proof. Backup verification checks integrity, then compares source and backup metadata and typed stored rows in lockstep, including rowid/primary-key identity and persisted FTS5 shadow rows. Unsupported virtual-table modules and tables shadowing every rowid alias fail closed. Backup creation, integrity, equivalence, fsync and proof hashing share a 900-second deadline.
Only a true first install with absent database and no managed predecessor,
installed binary/plist or loaded service omits the backup: private proof records
`database_source: absent`, `backup: null`, with path absence rechecked before
the boundary. It checks bounded first-install capacity instead; neither path
guarantees migration peak space. Maintenance excludes external database writers
throughout, including after handoff when the writer slot is released. It records
boundary attempted before binary replacement or candidate migration, durably
records `migration_started`, and runs exact candidate `--migrate-only` once with
600-second bound and inherited log output. It boots the private plist, verifies
stable new PID and exact version/full SHA, publishes the plist, and commits.

After a preboundary failure it may privately restart only a positively captured
unchanged predecessor whose stop, PID absence, binary/plist identity and recovered
health/PID are verified; otherwise it retains quarantine, claim and evidence for
manual recovery. At or after an attempted or uncertain boundary it attempts stop
and retains failed ownership, diagnostic proof and either the existing database's
backup or first-install absence proof; it never automatically restores a database
or starts the predecessor. `prod status` reports transaction-matched proof, or unknown when unavailable, without inferring recovery from a
stop attempt. Committed cleanup remains best effort. There is no resume, repair,
finalization or replay. All active markers fence new operations regardless of
terminal status; old manifests remain diagnostic evidence only.

Installed-runtime restart remains a separate protocol-2 helper that preserves
installed state and verifies exact identity on a new PID. ADR-263 qualifies the
backup and preboundary recovery exclusions of ADR-230; other retired routes stay
retired.

## Requirement coverage

| Requirements | Current implementation / verification |
| --- | --- |
| REQ-LDD-001–007,009–012,029 | `launchd_prod_deploy`, helper `validate_pre_disruption_handoff`/`activate`, `_preflight_backup_capacity`, `_first_install_capacity`, `sqlite_writer_guard`, `create_verified_database_backup`, `_verify_logical_equivalence`, `_backup_proof`, `_reserve_before_boundary`; focused typed-row, row-identity, FTS5-shadow and fail-closed regressions; disposable populated-database scale-qualification harness |
| REQ-LDD-013 | Disposable native launchd harness rejects production resources |
| REQ-LDD-014–016 | Installed-state restart helper; 38 focused tests and real initiator-death journey |
| REQ-LDD-030–031 | `capture_predecessor`, `restore_predecessor`, `write_proof`, `prod status`; preboundary recovery proof and postboundary fail-stop, old-manifest refusal and UI/API unresolved-recovery block |
| REQ-LDD-032 | Claim-release, reporting and cleanup failures preserve committed status and the healthy runtime; warning output is best effort |
| REQ-LDD-008,017–028 | Withdrawn by ADR-230; no active guarantee |

## Verification and deliberate test retirement

Earlier focused results (protocol 2, not acceptance of protocol 3): helper 25;
installer 60; restart 38; harness safety five; ReleaseUpdatePanel 32; and
successful disposable launchd commit, partial-write failure, health failure and
restart after initiator SIGKILL. The focused helper suite passed 81 tests,
including typed stored-row, row-identity, FTS5-shadow, quoted-module classification,
primary-key collation/direction without temporary sorting and fail-closed comparisons.
Hosted scale run `44017228572` qualified implementation `87e96effb`: a
13,406,384,128-byte populated database completed backup, full integrity check,
typed comparison, fsync and full-file proof hashing in 354.674 of 600 seconds
on macOS ARM64 with SQLite 3.53.1. The fixture has 1,290,000 rows each in an
ordinary table, a `WITHOUT ROWID` table and FTS5, with persisted FTS5 shadows
and secondary indexes. This is disposable fixture/runner evidence, not a
universal throughput guarantee, native-launchd journey or production qualification.
Exact-head artifacts and broader CI receipts belong with the corresponding PR. Diagnostics emit a bounded stderr JSON summary with cumulative stage timings, stage-entry budget and ordinal row/cell progress on both success and failure; raw integrity/SQLite error content is sanitized. The disposable shape matrix covers baseline, 24 extra stored columns and denser small-payload rows on CPython 3.13.1 / SQLite 3.47.1. Hosted outcomes and their limitations are tracked with the PR; no production deadline-fitness claim follows from file size or a passing synthetic shape alone.

The 6,406-line `test_current_schema_prepared_upgrade.py` and old manifest,
Activation, schema114/123/128/129, repair, refresh and rollback classes are retired
with their withdrawn contracts. `test_launchd_deploy.py` shrinks from 7,357 to
1,257 lines. Their identity/source/configuration/concurrency and essential failure
coverage is carried by the helper, ordinary installer and disposable harness;
obsolete recovery promises are intentionally not reproduced. The unused Rust
controller and its prepared validation harness are retired separately, not adopted.
The Rust `current_schema_controller_accepts_compiled_123_and_128_shapes` and its
single-use embedded Python hook are retired with deployment-side schema admission;
migration128 data/constraint and migration-lineage tests remain. The maintained
macOS app workflow runs ordinary helper, installer, harness-safety and restart
tests instead of the removed prepared-upgrade module.

### Fifteen-minute candidate qualification (2026-11-22)

[ADR-272](../adrs/272_bound-exhaustive-backup-verification-to-fifteen-minutes.md)
qualifies ADR-263 with a shared 900-second backup-verification deadline; candidate
migration remains bounded separately at 600 seconds. The comparator avoids
per-cell tag decoding and tuple allocation without changing storage-class,
binary64, row-identity, schema or FTS5-shadow comparisons.

Helper SHA-256 `7a55695c633c49e5a9fa879e7dabac897f5087d7172c0532cc1e97110b5d9fca`
completed one retained-snapshot, distinct-image isolated full-path qualification
in 817.564 seconds (supervisor 817.972 seconds), including proof hashing, on
CPython 3.13.1 / SQLite 3.47.1. The source was 13,745,197,056 bytes; exhaustive
comparison covered 6,988,720 rows and 63,467,170 cells. Copy took 83.774 seconds,
integrity 403.330, metadata 0.081, comparison 316.863, fsync under 0.001 and
proof hashing 13.510. Source identity/header were unchanged, the worker was
reaped, and source/destination had no open handles after completion.

This is one observed pass, not a controlled comparator speedup estimate or a
universal deadline guarantee. It does not retroactively qualify the earlier
failed 900-second run. Independent integrated-source review found no blocker.
Hosted run [45005235506](https://github.com/scottopell/phoenix-ide/actions/runs/45005235506)
records a **failed matrix** for exact source
`c484f7dff8aa917d2ed7ba03a7e36b9aa91500d0`, not release readiness: baseline and wide fixtures
completed in 315.275 and 322.792 seconds, but dense failed closed at the shared
900-second deadline during comparison, before fsync/proof. Dense contained
15,685,000 rows in each principal table and was 13,439,737,856 bytes; it spent
49.684 seconds copying, 539.206 on integrity and 311.085 in comparison, reaching
50,084,010 fully matched rows and 115,892,119 attempted cells. Its failure
artifact retains content-free stage diagnostics; total verification elapsed is
null on verifier exceptions, so stage scope-exit durations are not completion
claims. Both macOS and Linux release-helper qualification jobs passed. Successful
hosted qualification, guarded integration, corrected release preparation and
fresh sole-executor rollout gates remain unmet. The historical hosted result
above belongs to its named earlier implementation. No production acceptance
follows from isolated or synthetic fixture success.

### Targeted dense remedy candidate

The first clean integration candidate added an equal-row shortcut only when all
typed values match and no cell is tagged REAL; REAL rows retain binary64 comparison.
That candidate preserved row ordering/identity, stored FTS5 shadow coverage,
per-row deadline checks, diagnostics and the shared 900-second bound. On CPython 3.13.1 / SQLite
3.47.1, eight alternating old/new pairs on small replicas of the hosted shapes
showed dense comparison medians of 0.0348→0.0296 seconds, baseline
0.0489→0.0443 and wide 0.1676→0.1310. These measurements justify targeted hosted
qualification only, not a full-size runtime prediction. The earlier isolated
pass used a different helper hash and does not qualify this shortcut.

The CI dispatch can select just `dense` while retaining the complete `all`
matrix. The dense payload remains 128 bytes per TEXT/BLOB, and all safety checks
and budgets are unchanged. Independent review found no code blocker; 111 focused
Python tests and four spec/task lanes passed. However, targeted run
[45009531119](https://github.com/scottopell/phoenix-ide/actions/runs/45009531119)
failed dense on `4f8ac34ae682c84603350fbcbba9ca80a3f4b957` with helper SHA-256
`f3725a2d47329bf4d41508f524809dc7eebea80485ad91a87301b2c873947e9b`.
The unchanged 13,439,737,856-byte fixture reached 54,925,671 matched rows and
125,575,441 attempted cells: copy took 34.061 seconds, integrity 513.438,
metadata 0.014 and comparison consumed the remaining 352.468 seconds before
timeout, with no fsync/proof completion. Different stage timings and censored
comparison progress do not establish a controlled full-size speedup.

The formatting-only follow-up `4e649b58e6cd7dfa78f1db71c26870961a3b5264` passed
all hosted CI lanes in run
[45009887069](https://github.com/scottopell/phoenix-ide/actions/runs/45009887069).
The browser candidate rechecks kernel process-group membership after malformed
`/proc` data, while retaining errors for malformed data on still-owned members
and uncertain membership queries. Native CI success does not waive dense failure.

The next comparator candidate fetches 16-row batches per side (plus one retained
fallback row pair) and uses whole-batch equality only when every runtime storage
tag excludes REAL.
Other batches retain rowwise typed comparison and exact mismatch counters.
Deadline checks bracket fetching and comparison; equal-batch progress is counted
before the post-comparison deadline check. Timeout progress is therefore
batch-granular, not an identical clock-call/progress trace to the rowwise loop.
Buffers are bounded by row count, not bytes; large values can still increase
memory use and the duration of an uninterruptible Python comparison. Fetch-ahead
can change which deadline or SQLite read failure precedes an earlier mismatch,
without permitting an invalid database to pass.

On pinned CPython 3.13.1 / SQLite 3.47.1, twelve alternating paired measurements
following two warmup pairs on 5,000-row-per-principal-table disposable fixtures
showed rowwise→batch comparison medians of dense 0.047224→0.040171 seconds,
baseline 0.071097→0.060929 and wide 0.213089→0.201491. Equal successful counters
were asserted. These are small comparison-only observations, not full-path
estimates. The batch helper SHA-256 is
`2c10785eb667c271d732a0646b58e9bf4a22c268e2f9b39be4403a590b0bdec0`;
115 focused Python tests passed, including batch-boundary mismatches, signed
zero, missing/extra rows and post-batch deadline failure. Independent exact-delta
review found no blocker before one targeted hosted run; exact-source hosted
dense/native qualification remain gates.

### Batch result and integer-tag candidate

The batch helper at `73b7be92a0d8a88f631a7a2205887038a57506f9` failed the
single targeted dense run
[45012823182](https://github.com/scottopell/phoenix-ide/actions/runs/45012823182).
Copy took 31.060 seconds and integrity 477.056 seconds; comparison exited after
391.864 seconds at the shared deadline, without fsync/proof success. Ordinary
exact-source [CI 45012827553](https://github.com/scottopell/phoenix-ide/actions/runs/45012827553)
and macOS app run 45012827512 passed. Native success does not qualify dense.

The earlier 115-test unittest result was not warning-clean: SQLite fixture
connections and the capacity-reader connection relied on garbage collection
for closing. Explicit connection ownership was added while preserving write
transactions and the open WAL-source fixture. Subsequently 117 focused tests
passed with explicit checks for ResourceWarning and unraisable exceptions.

The next candidate projects `unicode(typeof(column))` integer storage tags,
leaving values, byte text factories, binary64 REAL comparison, batch size,
counters, ordering and deadlines unchanged. It does not enable mmap.
On CPython 3.13.1 / SQLite 3.47.1, twelve alternating measured pairs after two
warmups on actual 5,000-row-per-principal-table file-backed fixtures showed
comparison medians (milliseconds) dense 41.503→39.062, baseline 72.721→69.429,
and wide 215.051→194.744. Integrity timings were observationally similar, but
this microbenchmark used a read-only backup connection: its integrity timing is
not equivalent to the writable production check (see diagnostic limitations
below). These small warm-cache measurements do not establish full-path deadline fit.
The candidate helper SHA-256 is
`a50af20e662af0a20e5026df8bfa5c27d52bf6f2498879078f248d5ec172cadf`.
119 focused tests passed with no ResourceWarning/unraisable log matches,
including explicit storage-code/byte-value and unsupported-code diagnostics
coverage. Independent exact-delta review supported the targeted dense run
reported below; dense qualification and production assessment remain gates.

### Integer-tag terminal qualification

Exact candidate `547a0e11258620ee04a3d259bb352595063ff7ca` passed ordinary
[CI 45017168621](https://github.com/scottopell/phoenix-ide/actions/runs/45017168621)
and macOS app run 45017168612, but failed single targeted dense
[45017164489](https://github.com/scottopell/phoenix-ide/actions/runs/45017164489).
Copy took 45.132 seconds, full integrity 697.975 seconds and comparison exited
after 156.879 seconds at the shared deadline: 12,574,736 rows / 37,724,208 cells,
zero completed tables, ordinal 1; no fsync/proof success. Slower generation and
copy as well as integrity mean these different runs do not isolate a comparator
regression or prove runner contention. This is a performance/deadline failure,
not an established external-infrastructure blocker.

Subsequent pager-cache diagnostics left the production candidate unchanged.
Actual dense fixtures with 40,000 and 300,000 principal rows used respectively
69,328,896 and 512,106,496 aggregate main-file bytes across independent source
and backup images; temporary images were removed. Fresh connections, native
copy outside timing, unchanged full integrity/comparison, two warmup pairs and
twelve alternating measured pairs for the larger fixture compared observed
backup cache -2000 against -65536, with source settings and mmap unchanged.
The larger fixture's paired integrity wall ratio was 1.029413 (zero wins out of
twelve), and comparison ratio 1.006437 (two wins). CPU roughly equalled wall.
The earlier smaller fixture's integrity ratio was 0.982726. These warm-cache
measurements do not support a cache-only correction or establish scale behavior;
no cache tuning or additional full-size rerun followed. Dense full-path success
and fresh production assessment remain mandatory gates.

### Rejected overlap diagnostic and integrity-connection limitation

On CPython 3.13.1 / SQLite 3.47.1, a disposable CHECK-violating database was
rejected by integrity checking through a writable backup connection but reported
`ok` through a read-only connection. Initial read-only overlap screens therefore
do not establish equivalent integrity coverage and cannot qualify production.
The integer microbenchmark above shares that limitation. Both pager-cache
screens, inspected separately, ran integrity on writable destination connections;
their negative results are not invalidated by this connection-mode finding.
The production helper uses writable destination integrity checking throughout.

A corrected overlap prototype used a separately owned writable connection and
was measured through actual `create_verified_database_backup` calls, including
copy, integrity, exhaustive comparison and fsync, under a shared 30-second
screening deadline. Both helpers rejected the synthetic CHECK violation.
At 300,000 principal rows, five measured pairs following two warmups per arm
showed sequential/overlap full-helper wall medians of 5.014/5.407 seconds;
median paired wall and CPU ratios were 1.080467 and 1.306689. At 40,000 rows,
wall medians were 0.655/0.706 seconds. Arms alternated with the prototype first
in each pair; these are warm-cache observations, not scale predictions.
The harness recorded no warnings or unraisable exceptions. It removed each
backup after its run and used temporary fixture ownership; it did not emit a
separate post-cleanup receipt. The prototype's focused suite had failed before
its final connection-mode correction and was not rerun afterward.

Overlap was rejected and its source/tests restored, not committed as a candidate.
No cache tuning or overlap implementation was retained. The integer-tag dense
failure remains historical evidence; the bounded diagnostic below did not waive
it. The later full-size TEXT-wrapper qualification is reported separately.

### Row-only TEXT-wrapper remedy candidate

The next candidate removes per-cell SQL storage-tag expressions. On the helper's
ordinary owned SQLite connections, row-only `text_factory` wraps raw TEXT bytes
in a one-element tuple, distinct from BLOB bytes, integer, REAL and NULL values.
Metadata retains its byte factory. Both sides are checked for REAL values before
batch or row equality shortcuts; fallback uses exact types and binary64 REAL
comparison. Nested cursor ownership closes both row cursors before restoring
factories, including execute, fetch, mismatch and deadline failures.

A bounded CPython 3.13.1 / SQLite 3.47.1 dense screen used 300,000 principal rows,
128-byte TEXT/BLOB payloads and no extra columns. The source was 256,053,248 bytes.
Two warmup pairs and five measured pairs alternated arm order. Actual-helper
wall medians were 5.142 seconds for integer tags and 4.707 seconds for the wrapper,
with matching 1,203,721 rows, 2,708,217 cells and eight completed stored tables.
The screen included native backup, full integrity on the writable backup,
exhaustive comparison and file/directory fsync under one 30-second deadline;
it did not include outer transaction-proof publication or qualify the 900-second
large-fixture path. Both helpers rejected CHECK corruption. Retained-cursor
witnesses, warning/unraisable reports and temporary-directory cleanup were clean.

Earlier exploratory attempts are retained as diagnostics, not successful runs:
an initial baseline fixture exceeded its source cap, and two cursor-witness
attempts stopped before generation because of harness selection/count mistakes.
A reduced 1,000-row baseline/wide comparator screen is only supporting evidence.
The tracked production function blocks match the closed scratch candidate;
the full file hashes differ because scratch includes script metadata.

The parent four-module regression passed 124 tests with ResourceWarning promoted
to errors and no resource-warning or unraisable matches. Independent exact-delta
static review found no actionable issue. These results supported one targeted
hosted qualification; they did not qualify deployment.

The [targeted dense job](https://github.com/scottopell/phoenix-ide/actions/runs/45029233626/job/137228906115)
subsequently passed on source `db576d9fcadb4eb11a5ffe5f83a7c50788c20091`,
helper SHA256 `ef126688b3b194bcb3ee7466011be66858a72f40a1f71b0d497825d9594fdb3e`.
On CPython 3.13.1 / SQLite 3.47.1 it verified a 13,439,737,856-byte dense fixture
with 15,685,000 rows per principal table and 128-byte TEXT/BLOB payloads:

| Stage | Seconds |
|---|---:|
| Native copy | 32.135 |
| Full writable integrity | 438.709 |
| Metadata | 0.003 |
| Exhaustive comparison | 319.638 |
| Fsync | 0.000213 |
| Proof hash | 14.672 |
| Shared verification interval | **805.162 / 900** |

Generation took 366.977 seconds outside the verification budget; writer-guard
acquisition took 0.001 seconds and the enclosing interval was 805.165 seconds.
The comparator completed all eight stored tables (table ordinal nine including
the virtual table), 62,934,642 rows and 141,593,413 cells. Proof covered all
13,439,737,856 bytes with SHA256
`389a6a3669a48f915883ada1ae5eff60e13ff3b95333b05a00429094c23e8a09`.
This is a current-source dense backup-and-hash success, not evidence of durable
transaction-proof publication, real service installation/migration or rollout
safety. No performance repeatability guarantee follows from a single runner.

The overall targeted workflow failed a separate UI assertion in
`ProductConversationPage`'s active-Close background-refresh test, not the dense
job. PR ordinary CI and macOS app checks passed on a synthetic merge whose tree
is byte-identical to the candidate. [Explicit ordinary CI](https://github.com/scottopell/phoenix-ide/actions/runs/45032119542)
passed on the candidate commit with dense disabled; checkout logs confirm the
candidate rather than the PR merge. Observed `main`
`db5f0a064a348b3927854c75b90aa5748c5a30e9` is an ancestor of the clean candidate;
no predecessor release/version/publication history is introduced. Fresh rollout
assessment and frozen-release application-boundary verification remain separate
gates; no deployment, merge or publication is claimed.

## Operator recovery

Inspect logs, transaction-matched proof, any verified backup or first-install
absence proof, and actual service/helper state; preserve the failed transaction.
A proven preboundary unchanged predecessor may already be serving, but failed
status alone does not prove this. Confirm exact helper absence and resolve target state before clearing a surviving marker. After
attempted/uncertain candidate boundary, an older binary requires its matching
offline database restore with all users stopped. See `LAUNCHD.md`; historical
helper commands are not recovery APIs.

## Path and recovery identity correction

SQLite capacity, backup source and writer reservation use encoded file URIs. Regression fixtures include URI metacharacters and control characters in directory and database names, verify actual rows and writer exclusion, and reject missing sources without creating siblings. Preboundary predecessor recovery publishes and bootstraps the canonical plist so the loaded path satisfies the unchanged normal-restart same-file check. A composition regression executes the actual restart validator with mocked loaded-path inspection; it is not a live-launchd recovery/restart qualification. ADR-265 records the ordering correction and the original fixture blind spots.
