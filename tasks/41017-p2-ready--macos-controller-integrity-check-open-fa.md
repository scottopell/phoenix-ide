# Diagnose macOS controller integrity-check failure for compiled schema fixture

Observed while validating standalone browser CLI work: the unchanged database test `migrations::tests::current_schema_controller_accepts_compiled_123_and_128_shapes` fails both under `./dev.py check` and an isolated `cargo test -p phoenix-db current_schema_controller_accepts_compiled_123_and_128_shapes -- --nocapture` on macOS.

The Python controller imports `scripts/launchd_deploy_helper.py`, opens the generated fixture, then reports `sqlite3.OperationalError: unable to open database file` from `PRAGMA integrity_check` in `validate_database`. The traceback is wrapped as `ActivationError: SQLite integrity check failed`. The broad Rust lane stopped after 547 passing tests; no full-suite success was claimed. Neither the database crate nor controller script changed in the browser PR.

Investigate the fixture lifecycle, Python/SQLite build, readonly connection behavior, and temporary-file requirements. Establish the root cause with a focused regression and retain integrity checking rather than bypassing validation. The browser implementation and its focused tests do not require changing this database boundary.

## Isolated trigger and test hardening (2026-10-30)

The Rust fixture is fully checkpointed and its pool is closed before Python validation. The remaining main database still has WAL journal mode, but the WAL/SHM sidecars have been removed by the clean Rust close.

On identical copies of this preserved database:

- pyenv Python 3.12.8 and Xcode Python 3.9.6, both linked to Apple SQLite 3.51.0, fail even a schema SELECT through `mode=ro`, with `SQLITE_CANTOPEN` (14). The containing directory is writable. Read-write access succeeds, but switching the production validator to read-write would weaken its boundary and is not the fix.
- uv-managed Python 3.13.14 with SQLite 3.53.1 and Homebrew Python 3.15.0 with SQLite 3.54.1 both pass read-only integrity and foreign-key checks on the same file. Python's version alone is not the capability distinction; the linked SQLite build matters.

The regression now verifies exact returned ledgers, rejects a real schema with its required source table removed, asserts successful WAL checkpoint completion, reports Python/SQLite identities and extended error details, and retains the fixture directory on controller failure. It continues to expose the Apple SQLite failure rather than changing journal mode, using immutable reads, or selecting another interpreter internally.

The local `dev.py` uv environment also uses the pyenv 3.12.8 interpreter. Deployment captures `sys.executable` for its helper, so this is a controller-runtime capability issue as well as a test-environment issue. No production database or live deployment was exercised. Remaining work is to qualify/select a controller runtime that supports read-only WAL files without sidecars, with an early capability check; do not assume a Python version floor alone establishes that support.

Validation with uv-managed Python 3.13.14 / SQLite 3.53.1: the hardened Rust boundary test passes, including with inherited Python optimization and a poisoned Python module path; no-op and schema-bypass validator mutations both fail the intended assertions. All 224 current-schema Python tests and the separate unmocked immutable-release-pin round-trip pass. The synthetic unit fixtures bind their own registry digests rather than comparing mutable Rust test-source bytes to immutable release pins; production pins remain unchanged.
