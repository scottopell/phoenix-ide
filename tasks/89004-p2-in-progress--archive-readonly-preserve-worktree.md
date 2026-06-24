# Archived conversations should render read-only in the UI

## Scope

This task is **UI-only**.

Backend archive/delete cleanup is not part of this task. Current archive behavior already uses the shared cleanup cascade with the existing ownership rule: it cleans resources owned by the archived conversation’s `WorkScope`, and preserves a scope when another non-terminal, non-archived conversation still owns that same scope.

That means worktree-backed modes (`Explore { worktree_path: Some }`, `Work`, `Branch`) use `WorkScope::Worktree(path)`, so the worktree/tmux/bash/browser resources are owned by that worktree scope. Direct and sub-agent Explore without a worktree use `WorkScope::Conversation(id)` and have no owned project worktree to clean.

## Problem

The backend can mark a conversation archived while leaving its `ConvState` as `idle` or another non-terminal-looking state. `ConversationPage.tsx` mostly gates interactive UI on `convStateForChildren.type`, not on `conversation.archived`.

Result: an archived idle conversation can still render as resumable in the UI, including the chat composer and terminal affordances, even though archive is a terminal/read-only lifecycle outcome.

There is a second UI consistency issue in the grounding panel. `DesktopLayout.tsx` currently derives:

- `effectiveCwd = activeConversation?.worktree_path ?? activeConversation?.cwd ?? '/'`
- `FileExplorerPanel.workScopeKey = activeConversation?.work_scope_key`

For an archived/cleaned-up conversation, those owned entities may be gone by design. The UI should not keep offering Files, Work scope, or other worktree/work-scope affordances that point at resources the lifecycle cleanup just removed.

## Desired behavior

Viewing an archived conversation should behave like viewing a cleaned-up/done conversation:

- no chat composer / textbox;
- no send controls;
- no terminal split-pane;
- no WorkControlBar message/resource actions;
- no grounding-panel affordances for owned resources that were cleaned up:
  - no Work scope section/badge when the conversation no longer owns a live scope;
  - no project file tree rooted at a removed worktree;
  - no mobile/desktop file-browse button for a removed worktree;
- history, messages, metadata, and read-only views remain visible.

Do not hide durable/global grounding that is not owned by the archived work scope unless it also depends on the removed root. Examples: skills/MCP may remain if they are still meaningful for read-only context; task/history/message views should remain visible.

## Current UI points to inspect

- `ui/src/pages/ConversationPage.tsx`
  - composer branches around `ConnectedInputArea` currently key off `convStateForChildren.type`;
  - `showTerminal` currently excludes only `terminal`, `handed_off`, and `context_exhausted` states;
  - WorkControlBar render sites should also respect archived/read-only status;
  - mobile `StateBar` file button and `FileBrowserOverlay` should not be offered for removed owned worktrees.
- `ui/src/components/DesktopLayout.tsx`
  - computes `effectiveCwd` from `worktree_path` first;
  - always passes `workScopeKey` and `liveWorkScope` for the active conversation;
  - should derive a grounding/read-only capability model instead of blindly passing owned work-scope handles for archived rows.
- `ui/src/components/FileExplorer/FileExplorerPanel.tsx`
  - collapsed rail always shows “Files”;
  - expanded panel always renders `FileTree` rooted at `rootPath`;
  - renders `WorkScopeSection` whenever `workScopeKey` is present.
- `ui/src/components/StateBar.tsx`
  - file browse button renders whenever `onOpenFiles` exists.
- `ui/src/components/InputArea.tsx`
  - likely no backend change needed; parent should not render it for archived conversations.

## Implementation sketch

1. Derive one read-only lifecycle flag near the existing conversation/phase derivations:
   - `const isArchived = conversation.archived === true;`
   - optionally `const conversationReadOnly = isArchived || ...existing terminal checks...` if that makes branches clearer.

2. Gate conversation interactive UI with that flag:
   - never render `ConnectedInputArea` when `isArchived`;
   - never render `WorkControlBar` when `isArchived`;
   - make `showTerminal` false when `isArchived`;
   - ensure archived `error` / `awaiting_recovery` states do not expose resume composer controls.

3. Gate grounding-panel owned resources:
   - derive whether the active conversation has an available file root.
     - For archived worktree-backed rows, assume the owned `worktree_path` may be gone and do not expose it as a browseable root unless there is an explicit server-backed existence signal.
     - Direct mode has no owned worktree; if the user’s original `cwd` still represents durable project context, decide whether file browsing should remain read-only or be hidden for consistency. Prefer a clear capability flag over mode-specific guessing.
   - pass `undefined`/`null` for `workScopeKey` and `liveWorkScope` when the archived lifecycle has cleaned owned work-scope resources.
   - let `FileExplorerPanel` hide Files rail/tree when no file root is available, while keeping non-owned sections that still make sense.
   - hide the StateBar file button and `FileBrowserOverlay` when no file root is available.

4. Add UI regression tests:
   - archived idle conversation does not show textbox/send controls;
   - archived idle conversation does not show terminal pane;
   - archived worktree-backed conversation does not show grounding Files or Work scope affordances for the removed owned worktree;
   - archived conversation still renders existing message history;
   - non-archived idle conversation still shows composer, terminal, files, and Work scope behavior as before.

5. Specs/docs only if needed:
   - If a UI/conversation spec already states archive is read-only, link tests to it.
   - If not, add a small UI requirement that archived conversations are read-only in the page and grounding surfaces. Do not change backend cleanup semantics in this task.

## Acceptance checks

- Open an archived idle conversation: message history visible; chat input hidden; send hidden; terminal hidden.
- Open an archived worktree-backed conversation after cleanup: grounding panel does not offer stale Files/Work scope affordances for the removed owned worktree.
- Open a non-archived idle conversation: composer, terminal, grounding Files, and Work scope behavior unchanged.
- Open terminal/cleaned-up conversations: existing read-only behavior unchanged, and grounding does not point at removed owned resources.
- Backend archive/delete cleanup behavior unchanged.
- `./dev.py check` passes.
