# Diagnose macOS controller integrity-check failure for compiled schema fixture

Observed while validating standalone browser CLI work: the unchanged database test `migrations::tests::current_schema_controller_accepts_compiled_123_and_128_shapes` fails both under `./dev.py check` and an isolated `cargo test -p phoenix-db current_schema_controller_accepts_compiled_123_and_128_shapes -- --nocapture` on macOS.

The Python controller imports `scripts/launchd_deploy_helper.py`, opens the generated fixture, then reports `sqlite3.OperationalError: unable to open database file` from `PRAGMA integrity_check` in `validate_database`. The traceback is wrapped as `ActivationError: SQLite integrity check failed`. The broad Rust lane stopped after 547 passing tests; no full-suite success was claimed. Neither the database crate nor controller script changed in the browser PR.

Investigate the fixture lifecycle, Python/SQLite build, readonly connection behavior, and temporary-file requirements. Establish the root cause with a focused regression and retain integrity checking rather than bypassing validation. The browser implementation and its focused tests do not require changing this database boundary.
