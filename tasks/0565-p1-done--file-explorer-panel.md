---
id: 565
priority: p1
status: done
title: File Explorer Panel Implementation
created: 2025-02-22
requirements:
  - REQ-FE-001
  - REQ-FE-002
  - REQ-FE-003
  - REQ-FE-004
  - REQ-FE-005
  - REQ-FE-006
  - REQ-FE-007
  - REQ-FE-008
  - REQ-FE-009
  - REQ-FE-010
spec: specs/file-explorer/
---

# File Explorer Panel Implementation

Implement the File Explorer Panel feature for desktop, and refactor file browsing to use a single FileTree component across desktop and mobile.

## Source of Truth

**Read these specs thoroughly before implementing:**

- `specs/file-explorer/requirements.md` — EARS-format requirements (REQ-FE-001 through REQ-FE-010)
- `specs/file-explorer/design.md` — Architecture, component structure, state management
- `specs/file-explorer/executive.md` — Status tracking
- `specs/prose-feedback/requirements.md` — Related prose reader requirements (REQ-PF-005 onwards)

## Overview

This feature adds a persistent file explorer panel to the desktop layout, creating a three-column layout:

```
┌────────────┬────────────┬──────────────────────────────┐
│ Sidebar    │ FileExplorer │ Main Content                 │
│            │              │                              │
│ [Phoenix]  │ ▼ src/       │  ConversationPage            │
│ [+ New]    │   main.rs    │  OR                          │
│            │   lib.rs     │  ProseReader                 │
│ * conv-1   │ ▶ tests/     │                              │
│   conv-2   │   README.md  │                              │
│            │              │                              │
│ [<]        │ [<]          │                              │
└────────────┴────────────┴──────────────────────────────┘
```

Key concepts:
- **FileTree** is the core component, used in both desktop panel and mobile overlay
- **FileExplorerPanel** hosts FileTree on desktop (persistent, collapsible)
- **FileBrowserOverlay** hosts FileTree on mobile (modal overlay)
- **ProseReader** renders in main content area on desktop (not overlay)
- **FileBrowser.tsx** is deleted — replaced by the new components

## Implementation Checklist

### Phase 1: Extract FileTree Component

- [x] Create `ui/src/components/FileExplorer/FileTree.tsx`
- [x] Extract tree rendering logic from existing `FileBrowser.tsx`:
  - Directory listing via `/api/files/list`
  - Expand/collapse directories
  - File type icons (reuse `FileIcon` component)
  - Sorting (directories first, alphabetical)
  - Click handling for files and directories
- [x] FileTree props interface:
  ```typescript
  interface FileTreeProps {
    rootPath: string;
    onFileSelect: (filePath: string) => void;
    activeFile?: string;  // Highlight currently open file
    conversationId: string;  // For expansion state persistence
  }
  ```
- [x] Implement per-conversation expansion state persistence (localStorage)
- [x] Expansion state survives conversation switching (REQ-FE-002)

### Phase 2: Create Desktop Panel

- [x] Create `ui/src/components/FileExplorer/FileExplorerPanel.tsx`
- [x] Panel contains:
  - Header with "Files" title and collapse toggle
  - FileTree component
  - Collapse toggle button at bottom
- [x] Implement collapsed state (REQ-FE-005):
  - Narrow strip (~48px)
  - Shows RecentFilesStrip
  - Expand toggle button
- [x] Persist collapse state to localStorage (REQ-FE-004)

### Phase 3: Recent Files Strip

- [x] Create `ui/src/components/FileExplorer/RecentFilesStrip.tsx`
- [x] Create `ui/src/hooks/useRecentFiles.ts`
- [x] Track last 5 opened files per conversation (REQ-FE-006)
- [x] Store in localStorage: `phoenix:recent-files:{conversationId}`
- [x] Display as vertical stack of file type icons
- [x] Click icon opens file in prose reader (without expanding panel)

### Phase 4: Update Desktop Layout

- [x] Modify `ui/src/components/DesktopLayout.tsx`
- [x] Add FileExplorerPanel as middle column (REQ-FE-001)
- [x] Layout: `[Sidebar ~280px] [FileExplorer ~250px] [Main flex:1]`
- [x] Both sidebar and file explorer independently collapsible (REQ-FE-007)
- [x] Enforce minimum main content width (~400px)
- [x] CSS transitions for collapse/expand

### Phase 5: FileExplorer Context

- [x] Create `ui/src/components/FileExplorer/FileExplorerContext.tsx`
- [x] Context provides:
  ```typescript
  interface FileExplorerContextValue {
    openFile: (path: string) => void;
    activeFile: string | null;
    closeFile: () => void;
  }
  ```
- [x] Provider wraps DesktopLayout
- [x] FileExplorerPanel and main content area consume context

### Phase 6: Prose Reader Integration

- [x] Modify `ui/src/pages/ConversationPage.tsx`
- [x] Lift prose reader state to context or page level
- [x] When file is opened:
  - Desktop: Render ProseReader in main content area (replacing conversation)
  - Sidebar and FileExplorer remain visible
- [x] When prose reader closes:
  - Return to conversation view
- [x] Highlight active file in FileTree (REQ-FE-009)
- [x] Sending notes closes prose reader and returns to conversation (REQ-FE-008)

### Phase 7: Mobile Overlay

- [x] Create `ui/src/components/FileExplorer/FileBrowserOverlay.tsx`
- [x] Modal overlay that hosts FileTree (REQ-FE-010)
- [x] Header with path display and close button
- [x] Dismiss via close button or backdrop tap
- [x] On file select: open ProseReader overlay, close FileBrowserOverlay
- [x] Update file browse trigger in InputArea to use new overlay

### Phase 8: Cleanup

- [x] Delete `ui/src/components/FileBrowser.tsx`
- [x] Update any imports that referenced FileBrowser
- [x] Verify mobile file browsing still works
- [x] Verify patch file links still open prose reader correctly

### Phase 9: Styling

- [x] Create `ui/src/components/FileExplorer/FileExplorer.css`
- [x] Style expanded panel (~250px width)
- [x] Style collapsed panel (~48px width)
- [x] Style RecentFilesStrip (icon stack)
- [x] Active file highlight in tree
- [x] Loading indicators for directory expansion
- [x] Smooth collapse/expand transitions

## Files to Create

```
ui/src/components/FileExplorer/
├── index.ts
├── FileTree.tsx              # Core tree component
├── FileExplorerPanel.tsx     # Desktop host panel
├── FileBrowserOverlay.tsx    # Mobile host overlay
├── RecentFilesStrip.tsx      # Collapsed state recent files
├── FileExplorerContext.tsx   # Context for file open/close
└── FileExplorer.css

ui/src/hooks/
└── useRecentFiles.ts
```

## Files to Modify

- `ui/src/components/DesktopLayout.tsx` — add third column
- `ui/src/pages/ConversationPage.tsx` — prose reader in main content
- `ui/src/components/InputArea.tsx` — update file browse trigger

## Files to Delete

- `ui/src/components/FileBrowser.tsx` — replaced by FileTree + hosts

## Testing the Implementation

### REQ-FE-001: Three-Column Desktop Layout
```
1. Load app on desktop (> 1024px)
2. ✓ Three columns visible: Sidebar, FileExplorer, Main Content
3. Resize to < 1024px
4. ✓ FileExplorer panel hidden, only Sidebar + Main Content
```

### REQ-FE-002: File Tree Display
```
1. View file explorer panel
2. ✓ Shows tree rooted at conversation's cwd
3. Click a folder
4. ✓ Folder expands, shows contents
5. Switch to different conversation
6. ✓ Tree updates to new cwd
7. Switch back to first conversation
8. ✓ Previous expansion state restored
```

### REQ-FE-003: File Selection
```
1. Click a .md or .rs file in tree
2. ✓ ProseReader opens in main content area
3. ✓ Sidebar and FileExplorer remain visible
4. Click a non-text file (image, binary)
5. ✓ File appears disabled, not clickable
```

### REQ-FE-004: Panel Collapse - Expanded
```
1. Panel shows full file tree
2. ✓ Collapse toggle visible
3. Click collapse toggle
4. ✓ Panel collapses to narrow strip
5. Refresh page
6. ✓ Panel still collapsed (persisted)
```

### REQ-FE-005: Panel Collapse - Collapsed
```
1. Collapse the file explorer panel
2. ✓ Shows recent file icons (if any files opened)
3. ✓ Shows expand toggle
4. Click a recent file icon
5. ✓ File opens in prose reader
6. ✓ Panel stays collapsed
7. Click expand toggle
8. ✓ Panel expands to full tree
```

### REQ-FE-006: Recent Files Tracking
```
1. Open file A, then file B, then file C
2. Collapse panel
3. ✓ Recent files show C, B, A (most recent first)
4. Open file A again
5. ✓ Recent files show A, C, B (A moved to top)
6. ✓ Maximum 5 files shown
```

### REQ-FE-007: Accordion Panel Behavior
```
1. Collapse sidebar (conversation list)
2. ✓ File explorer and main content expand
3. Collapse file explorer
4. ✓ Main content expands further
5. Both panels collapsed
6. ✓ Main content has most space
7. ✓ Both collapse states persist independently
```

### REQ-FE-008: Prose Reader Integration
```
1. Click file to open prose reader
2. ✓ ProseReader in main content, sidebar + file explorer visible
3. Add annotation notes in prose reader
4. Click "Send" to send notes
5. ✓ Notes injected into input
6. ✓ Prose reader closes
7. ✓ Conversation view restored
```

### REQ-FE-009: Visual Feedback
```
1. Open a file in prose reader
2. ✓ That file is highlighted in file tree
3. Close prose reader
4. ✓ Highlight removed
5. Expand a folder (while loading)
6. ✓ Loading indicator shown on folder
```

### REQ-FE-010: Mobile File Browser Overlay
```
1. Resize to mobile viewport (< 768px)
2. Tap file browse button in conversation
3. ✓ Modal overlay appears with file tree
4. ✓ Header shows path and close button
5. Navigate folders, select a file
6. ✓ Prose reader opens (full screen overlay)
7. ✓ File browser overlay closes
```

## Completion Checklist

Before marking this task done, verify ALL of the following:

- [x] All implementation checklist items complete
- [x] All 10 requirements tested per "Testing the Implementation" section
- [x] FileBrowser.tsx deleted, no remaining imports
- [x] No TypeScript errors
- [x] No console errors during normal operation
- [x] Desktop three-column layout works at 1024px+
- [x] Mobile file browsing works (overlay)
- [x] Expansion state persists per conversation
- [x] Recent files persist per conversation
- [x] Panel collapse state persists globally
- [x] Prose reader opens in main content on desktop
- [x] Prose reader opens as overlay on mobile
- [x] Patch file links still work (open prose reader with highlights)
- [x] Update `specs/file-explorer/executive.md` status for each REQ-FE-*

## Out of Scope

- Keyboard navigation in file tree (future enhancement)
- File search/filter within tree
- Git status indicators
- File creation/deletion/rename
