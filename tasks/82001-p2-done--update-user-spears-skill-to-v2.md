# Migrate repo-local spEARS skills to canonical skills directory

The user-level spEARS skill update was delegated outside this Phoenix worktree. This task now covers the repo-local skill layout cleanup.

## Outcome

- Confirmed `spears-v2-migrate` already lives in the canonical repo `skills/` directory.
- Added `.agents/skills/spears-v2-migrate -> ../../skills/spears-v2-migrate` so agents discover it via the project skill directory convention.
- Added the same missing symlink for `phoenix-extract-crate`, which also already lived under `skills/` but was not exposed through `.agents/skills`.
- Confirmed there is no tracked `.claude/skills` directory left in this worktree.
