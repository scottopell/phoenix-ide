# Native macOS launchd Deployment — Executive Status

## Current reality

The protocol-3 deploy helper implements ordinary local/published deployment. It
validates exact candidate artifacts and checks backup capacity for an existing
database before disruption; it quarantines the canonical plist and observes
predecessor stop. For an existing database it holds a bounded SQLite
`BEGIN IMMEDIATE` writer reservation across backup verification and durable
boundary recording, retaining the fsynced native backup and transaction proof. Backup verification checks integrity, then compares source and backup metadata and typed stored rows in lockstep, including rowid/primary-key identity and persisted FTS5 shadow rows. Unsupported virtual-table modules and tables shadowing every rowid alias fail closed. Backup creation, integrity, equivalence, fsync and proof hashing share a 600-second deadline.
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
restart after initiator SIGKILL. The current focused helper suite passed 73 tests,
including typed stored-row, row-identity, FTS5-shadow and fail-closed comparisons.
The 13.4 GB scale-qualification harness has not produced a hosted success result;
there is no native-launchd or production qualification claim for these changes.

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

Full TypeScript checking is blocked by older primary dependencies missing patched
micromark/mdast modules; no wire type shape changed. The disposable populated-
database qualification targets a 13.4 GB SQLite file but has no hosted success
result yet; this is not a universal 13 GB performance guarantee. No production
deployment was performed; production remains `9c5d471d`. Focused and disposable
results do not claim hosted/full-repository or production acceptance.

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
