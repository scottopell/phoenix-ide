# Restore Cmd+P file search to the active conversation workspace

## Problem

`Cmd+P` still opens the command palette and searches conversations, but file results from the active conversation’s working directory are missing or scoped incorrectly.

The likely regression is from the `cwd` immutability / worktree-path split:

- `CommandPalette` builds its file source from the sidebar `conversations` list and uses `conversation.cwd` as the file root.
- `/api/conversations/:id/files/search` also searches `conversation.cwd` directly.
- After making `cwd` immutable, the effective file workspace for managed Explore/Work/Branch conversations is represented by `conv_mode.worktree_path()` when present, not necessarily by `cwd`.
- The live conversation snapshot already carries `worktree_path`, but the palette only receives the conversation list, whose reference-stability cache intentionally ignores field-only changes unless `updated_at` changes.

Net effect: the file source can be absent, stale, or searching/opening files relative to the wrong root, leaving Cmd+P effectively conversation-only.

## Plan

1. **Define a single effective file root contract**
   - Backend: add/use a helper for conversation-scoped filesystem APIs: `conv.conv_mode.worktree_path().unwrap_or(&conv.cwd)`.
   - Direct conversations continue to use `cwd`; Work/Branch and managed Explore use their typed worktree path.
   - Avoid adding another ad-hoc representation of the same path.

2. **Fix backend file search**
   - Update `search_conversation_files` to walk the effective file root, not raw `conversation.cwd`.
   - Keep returned paths relative to the root actually searched.
   - Add regression coverage with a conversation whose `cwd` and `worktree_path` differ and files exist only under `worktree_path`.

3. **Fix Cmd+P file source root on the frontend**
   - Pass the active conversation snapshot from `DesktopLayout` into `CommandPalette` instead of deriving it only from the conversations list.
   - Compute `activeFileRoot = activeConversation.worktree_path ?? activeConversation.cwd`.
   - Build `FileSource` with that root so selecting a file opens the same root the backend searched.

4. **Add UI regression coverage**
   - Cover that `CommandPalette` creates file search when an active conversation has a `worktree_path`.
   - Cover that selecting a file opens `${worktree_path}/${relativePath}`, not `${cwd}/${relativePath}`.

5. **Validate**
   - Run targeted Rust tests for the file search helper/handler.
   - Run targeted UI tests for CommandPalette/FileSource.
   - Run `./dev.py check` if the targeted tests pass.

## Acceptance

- In a Work/Branch/managed Explore conversation, `Cmd+P` searches files in the active worktree.
- Conversation results remain present in Cmd+P.
- Selecting a file result opens the file explorer at the correct root.
- Direct conversations still search `cwd`.
- Regression tests fail on the current `cwd`-only implementation and pass after the fix.
