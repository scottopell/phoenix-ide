Migrations are not transactional; a crash mid-migration bricks startup.

phoenix-db migrations.rs ~537 runs `raw_sql(migration.sql)` (multi-statement batch) outside any transaction, and the _migrations version INSERT is a separate statement. A crash between statements of a multi-step migration (e.g. MIGRATION_019 two RENAME COLUMNs, or after MIGRATION_011 ADD COLUMN) leaves it partially applied but unrecorded; re-run then fails (no such column / duplicate column), run_pending_migrations errors, and main.rs aborts startup — manual DB surgery required. Separately MIGRATION_014 json_set(content,...) errors on any malformed content JSON row, blocking the whole upgrade.

Fix: wrap each migration SQL + its _migrations INSERT in one transaction.

Found in spiritual-core audit 2026-06-10.
