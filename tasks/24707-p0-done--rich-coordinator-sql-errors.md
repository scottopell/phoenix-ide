# Return rich Coordinator SQL diagnostics

The Coordinator query boundary collapses unknown tables, unknown columns, malformed SQL, and SQLite contention into opaque `statement preparation failed` / `query execution failed` messages. Production shows repeated invalid schema guesses because the Coordinator cannot correct its queries.

## Acceptance criteria
- Non-policy SQLite failures expose operation phase, primary and extended result codes, symbolic code name, engine diagnostic, and error offset when available.
- Authorizer denials and budget exhaustion retain precedence and do not leak protected-object diagnostics.
- Unknown table, unknown column, syntax, and busy/locked failures have focused regression coverage.
- Existing global-recall specification documents actionable engine diagnostics.
