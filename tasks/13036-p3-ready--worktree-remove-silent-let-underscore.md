`runtime/executor.rs:559-562` discards the result of
`git worktree remove --force` with `let _ = ...`. There is an `info!`
*before* the call announcing the cleanup, but no log if the operation
fails. Failed cleanups leave orphan worktrees on disk indistinguishable
from successful ones.

## Verified location

`crates/phoenix-ide/src/runtime/executor.rs:550-563`

```rust
let worktree_path = repo_root
    .join(".phoenix")
    .join("worktrees")
    .join(&self.context.conversation_id);

if worktree_path.exists() {
    let worktree_str = worktree_path.to_string_lossy().to_string();
    tracing::info!(worktree = %worktree_str, "Cleaning up worktree on terminal");
    let _ = crate::git_ops::run_git(
        &repo_root,
        &["worktree", "remove", &worktree_str, "--force"],
    );
}
```

## Why this matters (capability-gaps-are-logged principle)

AGENTS.md: "When a component drops data because the backend does not
support a feature, this must appear in logs at debug level or above.
Silent omission is indistinguishable from a bug."

This is silent omission of an *error*, not a feature gap, but the same
test applies: the `info!` before the call announces intent; the absent
`warn!`/`error!` on failure makes a real failure (locked worktree, FS
permissions, inner repo, concurrent operation) invisible until a user
notices `.phoenix/worktrees/<conv>` accumulating on disk.

## Sibling pattern that does it right

`tools/browser/session.rs:617-619` (per the comment hunter's report)
follows the correct shape: announce, run, log the outcome.

## Fix direction

Replace `let _ =` with:

```rust
match crate::git_ops::run_git(
    &repo_root,
    &["worktree", "remove", &worktree_str, "--force"],
) {
    Ok(_) => {},
    Err(e) => tracing::warn!(
        worktree = %worktree_str,
        error = %e,
        "Failed to remove worktree on terminal -- may leak on disk",
    ),
}
```

Trivial change. No state-machine implications; this is post-terminal
cleanup that already accepts being best-effort.

## Related
- 08604 (projects-m4-merge-abandon, done -- worktree lifecycle context)
