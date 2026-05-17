Encode the conversation-cwd immutability contract beyond the single fixed site.

Task 02702 fixed Site 1 (the Explore→Work approve_task transition now promotes the Explore worktree in place, with a regression test in `cwd_immutability_tests`, executor.rs:3762) and was marked done. The remaining hardening work was deferred here:

- Site 2: `resolve_task` repo_root reset at executor.rs:~2250 still calls `update_conversation_cwd`.
- Site 3: startup recovery in main.rs still mutates cwd via `update_conversation_cwd`.
- Encode the contract so the mutation is not casually reachable: rename `update_conversation_cwd` (db.rs:667, traits.rs:119) to something like `update_conversation_cwd_recovery_only`, add a debug assertion guarding the legitimate call sites, and add a comment on the schema/column documenting that cwd is immutable post-creation except for recovery.

Decide per site whether the mutation is legitimate (recovery) and should be documented, or is a latent bug and should be removed.
