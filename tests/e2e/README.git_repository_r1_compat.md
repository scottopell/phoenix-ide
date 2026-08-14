# GitRepository Foundation paired-restore acceptance

`tests/e2e/git_repository_r1_compat.py` proves the supported rollback posture for the hidden GitRepository Foundation.

## Supported contract

Binary rollback is an **offline paired restore**:

1. stop Phoenix;
2. restore the pre-upgrade SQLite backup;
3. start the binary that matches that backup.

Phoenix does not promise that an older binary can open a database migrated by a newer binary. Database replacement while Phoenix is running is unsupported.

## What the runner proves

The runner:

- verifies the exact checked-in Project/shadow authority census;
- checks out and builds the pinned historical Phoenix revision in a detached temporary worktree;
- starts that historical binary on a historical-schema source database;
- creates one representative idle conversation and stops Phoenix;
- creates an offline SQLite backup and restores it to a separate database path;
- proves source, backup, and restored logical schema/value snapshots are identical;
- starts the same historical binary on the restored database;
- verifies exact historical runtime identity and completes an event-driven SSE read journey;
- proves the read journey did not mutate the restored database;
- runs current migration-65 and GitRepository reconciliation tests separately, failing if either filter executes zero tests.

The runner writes `target/git_repository_r1_compat.artifact.json` only after every assertion passes. On failure it removes any artifact and writes bounded historical server evidence to `target/git_repository_r1_compat.failure.log`.

## Run

```bash
./tests/e2e/git_repository_r1_compat.py
```

The runner requires an exactly clean checkout because it builds a pinned historical worktree and records the candidate HEAD.

Census detector self-test:

```bash
PHOENIX_R1_COMPAT_CENSUS_SELF_TEST=1 \
  uv run tests/e2e/git_repository_r1_compat.py
```

The harness is event-driven. Server startup waits for the emitted listening event, and conversation readiness waits for SSE `Idle`; wall-clock limits are outer liveness guards only.
