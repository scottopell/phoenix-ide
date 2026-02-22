# File Explorer Panel - Design Document

## Architecture Overview

The file explorer extends the desktop layout to three columns. It reuses existing FileBrowser logic but renders as a persistent panel instead of a modal overlay.

```
Desktop Layout (> 1024px):
┌────────────┬────────────┬──────────────────────────────┐
│ Sidebar    │ FileExplorer │ Main Content                   │
│            │              │                                │
│ [🔥]       │ ▼ src/       │  ConversationPage              │
│ [+ New]    │   main.rs    │  OR                            │
│            │   lib.rs     │  ProseReader                   │
│ ● conv-1   │ ▶ tests/     │                                │
│   conv-2   │   README.md  │                                │
│   conv-3   │              │                                │
│            │              ├──────────────────────────────┤
│            │              │  Input / StateBar              │
│ [◀]        │ [◀]          │                                │
└────────────┴────────────┴──────────────────────────────┘
  ~280px        ~250px              flex: 1


Collapsed State:
┌────┬────┬──────────────────────────────────────────┐
│ ●  │ 📄  │ Main Content (more space)                  │
│ ●  │ 📄  │                                            │
│ ●  │ 📄  │                                            │
│    │    │                                            │
│ [▶]│ [▶] │                                            │
└────┴────┴──────────────────────────────────────────┘
 48px  48px              flex: 1
```

## Component Structure

### DesktopLayout (REQ-FE-001, REQ-FE-007)

```typescript
function DesktopLayout({ children }: { children: React.ReactNode }) {
  const isDesktop = useMediaQuery('(min-width: 1024px)');
  const [sidebarCollapsed, setSidebarCollapsed] = useLocalStorage('sidebar-collapsed', false);
  const [fileExplorerCollapsed, setFileExplorerCollapsed] = useLocalStorage('file-explorer-collapsed', false);
  
  if (!isDesktop) return <>{children}</>;
  
  return (
    <div className="desktop-layout desktop-layout--three-col">
      <Sidebar collapsed={sidebarCollapsed} onToggle={() => setSidebarCollapsed(!sidebarCollapsed)} />
      <FileExplorerPanel collapsed={fileExplorerCollapsed} onToggle={() => setFileExplorerCollapsed(!fileExplorerCollapsed)} />
      <main className="desktop-main">{children}</main>
    </div>
  );
}
```

### FileExplorerPanel Component (REQ-FE-002, REQ-FE-004, REQ-FE-005)

```typescript
interface FileExplorerPanelProps {
  collapsed: boolean;
  onToggle: () => void;
}

function FileExplorerPanel({ collapsed, onToggle }: FileExplorerPanelProps) {
  const { conversation } = useCurrentConversation();
  const rootPath = conversation?.cwd || '/';
  const [recentFiles, setRecentFiles] = useRecentFiles(conversation?.id);
  
  if (collapsed) {
    return (
      <aside className="file-explorer file-explorer--collapsed">
        <RecentFilesStrip files={recentFiles} onFileClick={handleFileOpen} />
        <button onClick={onToggle} className="panel-toggle">▶</button>
      </aside>
    );
  }
  
  return (
    <aside className="file-explorer">
      <div className="file-explorer-header">
        <span className="file-explorer-title">Files</span>
        <button onClick={onToggle} className="panel-toggle">◀</button>
      </div>
      <FileTree rootPath={rootPath} onFileSelect={handleFileOpen} />
    </aside>
  );
}
```

### FileTree Component (REQ-FE-002, REQ-FE-003)

Refactored from existing FileBrowser, adapted for panel display:

```typescript
interface FileTreeProps {
  rootPath: string;
  onFileSelect: (filePath: string) => void;
  activeFile?: string;  // Currently open in prose reader
}

function FileTree({ rootPath, onFileSelect, activeFile }: FileTreeProps) {
  // Reuse existing FileBrowser logic:
  // - listFiles API calls
  // - expansion state management
  // - file type detection
  // - sorting (directories first, alphabetical)
  
  // Key differences from FileBrowser overlay:
  // - No header with navigation (always at rootPath)
  // - No close button
  // - Highlight activeFile
  // - Render inline, not as overlay
}
```

### RecentFilesStrip Component (REQ-FE-005, REQ-FE-006)

```typescript
interface RecentFilesStripProps {
  files: RecentFile[];
  onFileClick: (filePath: string) => void;
}

interface RecentFile {
  path: string;
  name: string;
  fileType: FileItem['file_type'];
  openedAt: number;
}

function RecentFilesStrip({ files, onFileClick }: RecentFilesStripProps) {
  // Show last 5 files as vertical stack of icons
  // Most recent at top
  // Click opens file in prose reader
  return (
    <div className="recent-files-strip">
      {files.slice(0, 5).map(file => (
        <button
          key={file.path}
          className="recent-file-icon"
          onClick={() => onFileClick(file.path)}
          title={file.name}
        >
          <FileIcon type={file.fileType} />
        </button>
      ))}
    </div>
  );
}
```

## State Management

### Panel Collapse State

```typescript
// localStorage keys
const SIDEBAR_COLLAPSED_KEY = 'sidebar-collapsed';
const FILE_EXPLORER_COLLAPSED_KEY = 'file-explorer-collapsed';

// Both panels independently collapsible
// State persists across sessions
```

### Recent Files State (REQ-FE-006)

```typescript
const RECENT_FILES_KEY = (convId: string) => `phoenix:recent-files:${convId}`;

function useRecentFiles(conversationId: string | undefined) {
  const [files, setFiles] = useLocalStorage<RecentFile[]>(
    conversationId ? RECENT_FILES_KEY(conversationId) : null,
    []
  );
  
  const addRecentFile = (file: RecentFile) => {
    setFiles(prev => {
      const filtered = prev.filter(f => f.path !== file.path);
      return [{ ...file, openedAt: Date.now() }, ...filtered].slice(0, 5);
    });
  };
  
  return [files, addRecentFile] as const;
}
```

### File Tree Expansion State (REQ-FE-002)

Expansion state persists per conversation and survives conversation switching:

```typescript
const EXPANSION_STATE_KEY = (convId: string) => `phoenix:file-tree-expansion:${convId}`;

function useExpansionState(conversationId: string | undefined) {
  const [expanded, setExpanded] = useLocalStorage<string[]>(
    conversationId ? EXPANSION_STATE_KEY(conversationId) : null,
    []
  );
  
  const expandedSet = useMemo(() => new Set(expanded), [expanded]);
  
  const toggleExpanded = (path: string) => {
    setExpanded(prev => 
      prev.includes(path) 
        ? prev.filter(p => p !== path)
        : [...prev, path]
    );
  };
  
  return { expandedSet, toggleExpanded };
}
```

## Prose Reader Integration (REQ-FE-008)

### Main Content Routing

The main content area switches between conversation and prose reader:

```typescript
function MainContent() {
  const [proseReaderFile, setProseReaderFile] = useState<ProseReaderState | null>(null);
  
  if (proseReaderFile) {
    return (
      <ProseReader
        filePath={proseReaderFile.path}
        rootDir={proseReaderFile.rootDir}
        onClose={() => setProseReaderFile(null)}
        onSendNotes={handleSendNotes}
        patchContext={proseReaderFile.patchContext}
      />
    );
  }
  
  return <ConversationPage />;
}
```

### Context for File Selection

File explorer needs to communicate with main content:

```typescript
interface FileExplorerContextValue {
  openFile: (path: string, rootDir: string) => void;
  activeFile: string | null;
  recentFiles: RecentFile[];
}

const FileExplorerContext = createContext<FileExplorerContextValue>(...);

// Provider wraps DesktopLayout
// FileExplorerPanel and MainContent both consume
```

## CSS Layout (REQ-FE-001, REQ-FE-007)

```css
.desktop-layout--three-col {
  display: flex;
  height: 100vh;
}

.sidebar {
  width: 280px;
  flex-shrink: 0;
  transition: width 0.2s ease;
}

.sidebar--collapsed {
  width: 48px;
}

.file-explorer {
  width: 250px;
  flex-shrink: 0;
  border-right: 1px solid var(--border-color);
  display: flex;
  flex-direction: column;
  transition: width 0.2s ease;
}

.file-explorer--collapsed {
  width: 48px;
}

.desktop-main {
  flex: 1;
  min-width: 400px;
  display: flex;
  flex-direction: column;
}

/* Ensure main content doesn't shrink too much */
@media (max-width: 1200px) {
  .file-explorer:not(.file-explorer--collapsed) {
    width: 200px;
  }
}
```

## Files to Create/Modify

### New Files

```
ui/src/components/FileExplorer/
├── index.ts
├── FileTree.tsx              # Core tree component (extracted from FileBrowser)
├── FileExplorerPanel.tsx     # Desktop host panel
├── FileBrowserOverlay.tsx    # Mobile host overlay (replaces FileBrowser)
├── RecentFilesStrip.tsx
├── FileExplorerContext.tsx
└── FileExplorer.css

ui/src/hooks/
└── useRecentFiles.ts
```

### Modified Files

- `ui/src/components/DesktopLayout.tsx` — add FileExplorerPanel column
- `ui/src/pages/ConversationPage.tsx` — lift prose reader state for context

### Deleted Files

- `ui/src/components/FileBrowser.tsx` — replaced by FileTree + host components

## Relationship to Existing Components

| Component | Role | Notes |
|----------|------|-------|
| `FileTree` | Core tree component | Single component, renders in panel (desktop) or overlay (mobile) |
| `FileExplorerPanel` | Desktop host | Hosts FileTree, handles collapse state |
| `FileBrowserOverlay` | Mobile host | Modal overlay hosting FileTree, replaces old FileBrowser |
| `FileBrowser` | Deprecated | Remove entirely; functionality split into FileTree + hosts |
| `ProseReader` | Unchanged | Renders in main content (desktop) or overlay (mobile) |
| `DesktopLayout` | Extended | Adds third column for FileExplorerPanel |
| `Sidebar` | Unchanged | Leftmost column in three-column layout |

## Mobile Behavior

On mobile (< 1024px):
- FileExplorerPanel is not rendered
- Existing FileBrowser overlay (REQ-PF-001) remains the file browsing method
- ProseReader continues to use full-screen overlay
- No changes to mobile UX
