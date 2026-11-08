# Preserve source during generic terminal lifecycle

Remove generic conversation-terminal worktree deletion because terminal state carries no exact `ProductConversation` Close authority. Exact Close and explicit hard-delete remain unchanged. Startup reconciliation must classify an ordinary Open or Close-fenced `ProductConversation` as the worktree owner even when its current transcript is terminal; terminal state alone must not make that worktree reclaimable.

Regression coverage must prove terminal lifecycle preserves clean, tracked-dirty, untracked, and nested descendant paths without invoking `git worktree remove --force`. Startup coverage must prove a clean worktree survives reconciliation while its ordinary aggregate is Open, while History and rowless fixtures continue to exercise the existing orphan-reclamation boundary. Remove any descendant-path heuristic that becomes dead with the unauthorized cleanup path. Do not redesign the Close lifecycle or reproduce against real work.
