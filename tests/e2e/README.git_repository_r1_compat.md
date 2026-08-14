# GitRepository R1 compatibility acceptance

Run `uv run tests/e2e/git_repository_r1_compat.py` only from an exact clean
candidate commit. This explicit, heavyweight acceptance runner builds the frozen
historical server in a detached temporary worktree, migrates an empty database
with the exact candidate binary from `ROOT/target`, and lets the historical
binary create a seeded-empty conversation through HTTP and SSE. The candidate's
ignored private finalizer then catches up the dormant GitRepository shadows.

The test-only artifact is `target/git_repository_r1_compat.artifact.json`. It is
acceptance evidence, **not R2 authorization and not Phoenix product
persistence**. It binds an independently recomputed length-framed integrity SHA
to the process-local run nonce, canonical target-database digest, candidate and
historical identities, compiled schema, complete source snapshots, and four
phase-specific shadow snapshot digests: before and after the initial historical
binary exercise, plus before and after its rollback exercise. Each phase pair
must be equal; the two phase digests may differ because candidate catch-up runs
between them. The additive schema is retained; destructive down-migration is
prohibited.

On failure the runner retains drained historical-server output at
`target/git_repository_r1_compat.failure.log`. It never uses sleeps or direct
historical SQL writes. The historical process is started in a process group and
is terminated/reaped with TERM then KILL escalation.
