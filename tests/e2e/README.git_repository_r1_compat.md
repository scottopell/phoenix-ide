# GitRepository R1 compatibility acceptance

Run `uv run tests/e2e/git_repository_r1_compat.py` from an exact clean candidate
commit. It builds the frozen historical server in a detached temporary worktree,
migrates an empty database with the candidate, and lets the old server create a
seeded-empty conversation through HTTP and SSE. The candidate's ignored private
finalizer then catches up the dormant GitRepository shadows.

The acceptance artifact is `target/git_repository_r1_compat.artifact.json`.
It is CI/acceptance evidence, never Phoenix product persistence. The acceptance
posture is additive schema retained; destructive down-migration is prohibited.
