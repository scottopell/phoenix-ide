# GitRepository R1 compatibility acceptance

Run `uv run tests/e2e/git_repository_r1_compat.py` only from an exact clean
candidate commit. This explicit, heavyweight acceptance runner builds the frozen
historical server in a detached temporary worktree without staging its ignored
UI placeholder, requires the exact clean 12-character historical identity,
migrates an empty database with the exact candidate binary from `ROOT/target`,
and lets the historical binary create a seeded-empty conversation through HTTP
and SSE. The candidate's
ignored private finalizer then catches up the dormant GitRepository shadows.

The test-only artifact is `target/git_repository_r1_compat.artifact.json`. It is
acceptance evidence, **not R2 authorization and not Phoenix product
persistence**. The artifact carries canonical serde `snake_case` typed readiness
(every diagnostic category and bounded sample, valid absence, storage kind,
applied migration ledger, and inspected R1 DDL) and the same canonical JSON is an
integrity member verified exactly by Python. It binds an independently recomputed length-framed integrity SHA
to the process-local run nonce, canonical target-database digest, candidate and
historical identities, compiled schema, complete source snapshots, and four
phase-specific shadow snapshot digests: before and after the initial historical
binary exercise, plus before and after its rollback exercise. Each phase pair
must be equal; the two phase digests may differ because candidate catch-up runs
between them. The additive schema is retained; destructive down-migration is
prohibited.

Preparation produces a typed, private artifact before rollback, binding the exact
candidate SHA/package/build schema, target database, complete source and initial
shadow digest, fresh preparation root/nonce, typed readiness digest, and both
catch-up statistics. Finalization re-derives and compares every binding before
minting its run-bound preparation attestation.
The runner also pins the exact preparation-file bytes before rollback;
finalization requires that independent SHA-256 pin, so no preparation root,
nonce, statistic, member, or top-level field can change between phases.

Published success evidence is removed at run start, verified against an exact
top-level shape and exhaustive member digest, then atomically installed without
reordering the verified Rust serialization. A failed run leaves no stale success
artifact and records only that run's server logs.

The authority census inventories explicit production Project readers, writers,
and paths across conversation persistence/inheritance, active-work eligibility,
WorkScope writes and cleanup, global read, usage, analytics, and direct SQL.
Run `PHOENIX_R1_COMPAT_CENSUS_SELF_TEST=1 uv run tests/e2e/git_repository_r1_compat.py`
to prove an injected occurrence changes the observed inventory.

On failure the runner retains only its drained historical-server output at
`target/git_repository_r1_compat.failure.log`; it removes stale failure output at
run start and does not create it on success. It never uses sleeps or direct
historical SQL writes. Startup is event-driven; only an early explicit bind race
gets a bounded fresh-port retry, with the old process fully reaped. The historical
process is started in a process group and is terminated/reaped with TERM then KILL
escalation. Worktree cleanup is mandatory and is grouped with any body failure.
