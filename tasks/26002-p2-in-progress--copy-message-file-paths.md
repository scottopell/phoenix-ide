# Add copy path actions to message file-path links

## Goal

Make clickable file paths in Phoenix messages support right-click actions:

- **Copy absolute path**
- **Copy relative path**

This should work for the existing clickable paths rendered in agent messages, including inline-code paths and paragraph/list text paths.

## Current implementation notes

- File paths are parsed and rendered by `ui/src/utils/linkify.tsx`.
  - `parseLinks()` detects URLs and file paths.
  - `linkifyText()` renders interactive file path spans with class `file-path-link`.
- Click-to-open is threaded through:
  - `AgentMessage` in `ui/src/components/MessageComponents.tsx`
  - `MessageList` in `ui/src/components/MessageList.tsx`
  - `ConversationPage.handleOpenFileFromPatch()`
- The conversation root is already known in `ConversationPage` as `conversation.worktree_path ?? conversation.cwd ?? '/'`.
- Clipboard behavior should reuse `ui/src/utils/clipboard.ts` rather than calling `navigator.clipboard` directly.
- Phoenix already has a message-level custom context menu in `ui/src/components/MessageContextMenu.tsx`; this task should avoid conflicts with that menu.

## Implementation plan

1. Add path normalization helpers in the UI layer, near the file path linkification code or in a small utility:
   - Given a displayed path and root directory, compute:
     - absolute path: existing absolute path unchanged; relative path resolved against root.
     - relative path: path relative to root when possible; otherwise use the displayed path or a safe normalized fallback.
   - Normalize duplicate slashes and trailing root slashes without changing meaningful path text.

2. Extend file-path link rendering so `linkifyText()` can receive optional path-copy context:
   - Keep the existing `onFileClick(filePath)` behavior unchanged.
   - Add an optional callback/config for right-click copy actions, including the conversation root directory.
   - Add data attributes if useful for event delegation/tests, e.g. displayed path, absolute path, relative path.

3. Add a small file-path-specific context menu:
   - Trigger on `contextmenu` for `.file-path-link`.
   - Show menu items:
     - `Copy absolute path`
     - `Copy relative path`
   - Use `copyToClipboard()`.
   - Close on outside click and Escape.
   - Clamp to viewport like the existing message context menu.
   - Ensure Shift+right-click still gives the native browser menu as an escape hatch.

4. Avoid collision with the existing message context menu:
   - When right-clicking a file path, prevent the message-level menu from opening.
   - Do not disable normal left-click open behavior or keyboard activation.

5. Thread the root directory to message rendering:
   - Add an optional `rootDir`/path-copy context prop from `ConversationPage` -> `MessageList` -> `AgentMessage` -> `linkifyText()`.
   - Use `conversation.worktree_path ?? conversation.cwd ?? '/'`, matching the existing open-file behavior.
   - Keep shared/read-only `SharePage` behavior unchanged or pass no root context there, depending on whether copy actions should appear in shared pages.

6. Tests:
   - Unit-test path resolution helpers for:
     - relative path under root
     - absolute path under root
     - absolute path outside root
     - root with trailing slash
   - Component-test that right-clicking a file path opens the path menu and copies absolute/relative values.
   - Regression-test that normal left-click still calls `onOpenFile`.
   - Regression-test that right-clicking non-path message text still opens the existing message context menu.

## Acceptance criteria

- Right-clicking a clickable file path in a conversation message opens a menu with `Copy absolute path` and `Copy relative path`.
- Copy absolute path writes the resolved absolute path to the clipboard.
- Copy relative path writes the path relative to the conversation root when the path is inside the root.
- Existing click-to-open behavior is unchanged.
- Existing message context menu behavior still works for non-file-path message content.
- UI tests cover copy behavior and context-menu interaction.
