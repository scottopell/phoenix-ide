# Prose Feedback UI - Technical Design

## Architecture Overview

The Prose Feedback feature consists of a full-screen overlay component (`ProseReader`) that:
1. Fetches and renders text file content
2. Enables line-level annotation via long-press gesture
3. Manages a session-local collection of notes
4. Formats and injects notes into the message input

This is a **frontend-only feature** - no backend API changes required beyond the existing file read endpoint.

## Component Architecture

### REQ-PF-001, REQ-PF-002, REQ-PF-003, REQ-PF-004 Implementation: FileBrowser Component

**Location:** `ui/src/components/FileBrowser.tsx`

The FileBrowser is a modal overlay that:
- Fetches directory listings from the backend
- Manages navigation state (current path)
- Handles file selection callbacks

**State:**
```typescript
interface FileBrowserState {
  currentPath: string;
  items: FileItem[];
  loading: boolean;
  error: string | null;
  expandedPaths: Set<string>; // Persisted per conversation
}

interface FileItem {
  name: string;
  path: string;
  isDirectory: boolean;
  size?: number;
  modifiedTime?: number;
  type: 'folder' | 'markdown' | 'code' | 'config' | 'text' | 'image' | 'data' | 'unknown';
  isTextFile: boolean; // Can be opened in prose reader
}
```

**API Integration:**
```typescript
// List directory contents
GET /api/files/list?path={currentPath}
Response: {
  items: [{
    name: string,
    path: string,
    isDirectory: boolean,
    size?: number,
    modifiedTime?: number
  }]
}
```

**File Type Detection:**
```typescript
const getFileType = (name: string): { type: FileType, isTextFile: boolean } => {
  const ext = name.split('.').pop()?.toLowerCase();
  if (!ext) return { type: 'text', isTextFile: true }; // No extension = text
  
  const typeMap = {
    markdown: { exts: ['md', 'markdown'], isText: true },
    code: { exts: ['rs', 'ts', 'tsx', 'js', 'jsx', 'py', 'go', 'java', 'cpp', 'c', 'h'], isText: true },
    config: { exts: ['json', 'yaml', 'yml', 'toml', 'ini', 'env'], isText: true },
    text: { exts: ['txt', 'log'], isText: true },
    image: { exts: ['png', 'jpg', 'jpeg', 'gif', 'svg', 'webp'], isText: false },
    data: { exts: ['db', 'sqlite', 'bin', 'dat'], isText: false }
  };
  
  for (const [type, { exts, isText }] of Object.entries(typeMap)) {
    if (exts.includes(ext)) return { type, isTextFile: isText };
  }
  
  // Unknown extension - will need content detection
  return { type: 'unknown', isTextFile: false };
};
```

**Sorting Logic:**
```typescript
items.sort((a, b) => {
  // Directories first
  if (a.isDirectory !== b.isDirectory) {
    return a.isDirectory ? -1 : 1;
  }
  // Then alphabetical (case-insensitive)
  return a.name.toLowerCase().localeCompare(b.name.toLowerCase());
});
```

### REQ-PF-005 Implementation: ProseReader Component

**Location:** `ui/src/components/ProseReader.tsx`

The ProseReader is a modal overlay that receives:
- `filePath`: Path to the file to display
- `rootDir`: Working directory for resolving relative paths  
- `onClose`: Callback when reader is closed
- `onSendNotes`: Callback receiving formatted notes string

**Note**: The component should compute and store the absolute file path by resolving `filePath` against `rootDir` for use in the formatted output.

File type detection uses extension mapping:
- Markdown: `.md`, `.markdown` → rendered via `react-markdown` with `remark-gfm`
- Code: `.rs`, `.ts`, `.tsx`, `.js`, `.jsx`, `.py`, `.go`, `.json`, `.yaml`, `.yml`, `.toml`, `.css`, `.html` → syntax highlighted via `react-syntax-highlighter`
- Text: all other extensions → attempt text decoding, show as monospace if valid UTF-8/UTF-16/ASCII

**Text encoding detection:**
```typescript
const isValidTextFile = async (content: ArrayBuffer): Promise<boolean> => {
  try {
    // Try UTF-8 first (most common)
    new TextDecoder('utf-8', { fatal: true }).decode(content);
    return true;
  } catch {
    try {
      // Try UTF-16
      new TextDecoder('utf-16', { fatal: true }).decode(content);
      return true;
    } catch {
      // Check if it's ASCII (bytes 0-127)
      const bytes = new Uint8Array(content);
      return bytes.every(byte => byte < 128);
    }
  }
};
```

### REQ-PF-006 Implementation: Long-Press Gesture

Gesture handling uses three touch event handlers per selectable element:
- `onTouchStart`: Start 500ms timer, store line info and initial touch position
- `onTouchMove`: Check if moved >10px from start position, cancel timer if so
- `onTouchEnd`: Cancel timer if still running

**Movement detection implementation:**
```typescript
const [touchStart, setTouchStart] = useState<{x: number, y: number} | null>(null);
const [longPressTimer, setLongPressTimer] = useState<number | null>(null);

const handleTouchStart = (e: TouchEvent, lineData: LineInfo) => {
  const touch = e.touches[0];
  setTouchStart({ x: touch.clientX, y: touch.clientY });
  
  const timer = window.setTimeout(() => {
    navigator.vibrate?.(50);
    openAnnotationDialog(lineData);
  }, 500);
  
  setLongPressTimer(timer);
};

const handleTouchMove = (e: TouchEvent) => {
  if (!touchStart || !longPressTimer) return;
  
  const touch = e.touches[0];
  const deltaX = Math.abs(touch.clientX - touchStart.x);
  const deltaY = Math.abs(touch.clientY - touchStart.y);
  
  // 10px threshold for cancellation - very sensitive to any movement
  if (deltaX > 10 || deltaY > 10) {
    window.clearTimeout(longPressTimer);
    setLongPressTimer(null);
  }
};
```

Timer fires → trigger haptic feedback (`navigator.vibrate(50)`) and open annotation dialog.

For markdown content, each rendered block (p, h1-h3, li, blockquote) is wrapped with gesture handlers. A line counter increments during render to assign logical line numbers.

For code/text content, `react-syntax-highlighter`'s `lineProps` callback attaches handlers to each line, or lines are rendered individually in a loop.

### REQ-PF-007 Implementation: Annotation Dialog

**State:**
- `annotatingLine: { lineNumber: number; lineContent: string } | null`
- `noteInput: string`

Dialog is a bottom-sheet overlay (slides up from bottom). Auto-focuses textarea on open via `useEffect` with ref.

Keyboard shortcuts handled in textarea's `onKeyDown`:
- Escape → close dialog
- Ctrl/Cmd+Enter → submit note

### REQ-PF-008 Implementation: Notes Management

**Note data structure:**
```typescript
interface ReviewNote {
  id: string;           // crypto.randomUUID()
  filePath: string;
  lineNumber: number;
  lineContent: string;  // Full raw line text (not truncated)
  note: string;         // User's annotation
  timestamp: number;    // Date.now() for ordering
}
```

**State:**
- `notes: ReviewNote[]`
- `showNotesPanel: boolean`
- `highlightedLine: number | null`

Jump-to-line uses a `Map<number, HTMLElement>` of refs registered during render. Scrolls element into view with `scrollIntoView({ behavior: 'smooth', block: 'center' })`. Highlight animation via CSS class with 2s timeout to remove.

### REQ-PF-009 Implementation: Notes Formatting and Injection

Format function:
```typescript
const formatted = `Review notes for \`${absoluteFilePath}\`:\n\n` +
  notes.map(n => {
    // Use the full raw line content for greppability
    return `> Line ${n.lineNumber}: \`${n.lineContent}\`\n${n.note}`;
  }).join("\n\n");
```

Note: The `lineContent` field stores the complete raw line text, not a truncated preview. This ensures the AI can search for exact matches in the codebase.

The `onSendNotes` callback passes this string to the parent component, which injects it into the message input state. The parent handles appending to existing draft with appropriate spacing.

### REQ-PF-010 Implementation: Close Confirmation

The `handleBack` function checks `notes.length > 0` before closing. Uses `window.confirm()` for simplicity, though a custom modal could be used for consistency.

### REQ-PF-011 Implementation: Session Scope

Notes state is local to the ProseReader component instance. When the component unmounts (on close), state is lost. No localStorage persistence is implemented - this is intentional per requirements.

### REQ-PF-012, REQ-PF-013 Implementation: Layout and States

CSS uses:
- `position: fixed; inset: 0` for full-screen overlay
- `height: 100dvh` for mobile viewport handling
- Bottom-sheet pattern with `animation: slide-up` for dialogs
- Loading/error states as centered flex containers

### REQ-PF-014 Implementation: Patch Output Integration

**Context:** This feature integrates with the patch tool's unified diff output (REQ-PATCH-007). The patch tool already generates diffs for UI display, so this implementation focuses on making those diffs interactive.

**Parsing Unified Diffs:**
```typescript
// Parse line numbers from unified diff format
const parseUnifiedDiff = (diffContent: string): Set<number> => {
  const modifiedLines = new Set<number>();
  const lines = diffContent.split('\n');
  let currentLine = 0;
  
  lines.forEach(line => {
    // @@ -start,count +start,count @@ format
    const hunkHeader = line.match(/@@ -\d+,?\d* \+(\d+),?(\d*) @@/);
    if (hunkHeader) {
      currentLine = parseInt(hunkHeader[1]) - 1;
      return;
    }
    
    if (line.startsWith('+') && !line.startsWith('+++')) {
      modifiedLines.add(currentLine + 1);
    }
    
    if (!line.startsWith('-')) {
      currentLine++;
    }
  });
  
  return modifiedLines;
};
```

**Detecting Files in Patch Output:**
```typescript
// Match filenames in unified diff headers
const renderPatchOutput = (content: string) => {
  // Pattern matches: --- a/path/to/file.ext or +++ b/path/to/file.ext
  const diffFileRegex = /^[+-]{3}\s+[ab]\/(.+)$/gm;
  
  return content.replace(diffFileRegex, (match, filePath) => {
    // Only make +++ lines clickable (the "after" state)
    if (match.startsWith('+++')) {
      return `<span class="patch-file-link" data-path="${filePath}">${match}</span>`;
    }
    return match;
  });
};
```

**ProseReader Props for Patch Mode:**
```typescript
interface ProseReaderProps {
  filePath: string;
  rootDir: string;
  onClose: () => void;
  onSendNotes: (notes: string) => void;
  // New props for patch integration
  patchContext?: {
    modifiedLines: Set<number>; // Line numbers that were modified
    firstModifiedLine?: number; // For auto-scrolling
  };
}
```

**Diff Highlighting CSS:**
```css
.prose-reader-line--modified {
  background-color: rgba(255, 236, 156, 0.3); /* Gentle yellow */
  border-left: 3px solid #f0ad4e;
}

.prose-reader-line--added {
  background-color: rgba(195, 232, 195, 0.3); /* Gentle green */
  border-left: 3px solid #5cb85c;
}

.prose-reader-line--deleted {
  background-color: rgba(255, 220, 220, 0.3); /* Gentle red */
  border-left: 3px solid #d9534f;
  text-decoration: line-through;
  opacity: 0.7;
}

.prose-reader-banner {
  background: #f8f9fa;
  padding: 8px 16px;
  border-bottom: 1px solid #dee2e6;
  font-size: 14px;
  color: #6c757d;
}
```

**Auto-scroll to First Change:**
```typescript
// In ProseReader component
useEffect(() => {
  if (patchContext?.firstModifiedLine) {
    const lineElement = lineRefs.get(patchContext.firstModifiedLine);
    lineElement?.scrollIntoView({ 
      behavior: 'smooth', 
      block: 'center' 
    });
  }
}, [patchContext?.firstModifiedLine]);
```

**Auto-prefix for Changed Line Notes:**
```typescript
const handleAddNote = (lineNumber: number, noteText: string) => {
  const isModifiedLine = patchContext?.modifiedLines.has(lineNumber);
  const finalNote = isModifiedLine && !noteText.startsWith("[Changed line]") 
    ? `[Changed line] ${noteText}`
    : noteText;
  
  addNote({
    lineNumber,
    note: finalNote,
    // ... other fields
  });
};
```

### Integration with Message Display

The conversation message component needs to detect and parse unified diffs from patch tool output:

```typescript
// In ConversationMessage component
useEffect(() => {
  // Add click handlers to patch file links
  const handleFileClick = (e: MouseEvent) => {
    const target = e.target as HTMLElement;
    if (target.classList.contains('patch-file-link')) {
      const filePath = target.dataset.path;
      if (filePath) {
        // Extract the diff section for this file from the message
        const diffContent = extractDiffForFile(message.content, filePath);
        const modifiedLines = parseUnifiedDiff(diffContent);
        const firstModified = Math.min(...Array.from(modifiedLines));
        
        openProseReader(filePath, {
          modifiedLines,
          firstModifiedLine: firstModified
        });
      }
    }
  };
  
  messageRef.current?.addEventListener('click', handleFileClick);
  return () => messageRef.current?.removeEventListener('click', handleFileClick);
}, [message]);
```

**Note:** This implementation leverages the existing unified diff output from REQ-PATCH-007, avoiding duplication of diff generation logic.

## Integration Points

### Conversation UI Integration

The conversation page needs a button to open the file browser. This could be:
- A button in the message input toolbar
- A menu item in a conversation actions menu
- A keyboard shortcut (e.g., Ctrl+O)

The parent component manages state:
```typescript
const [showFileBrowser, setShowFileBrowser] = useState(false);
const [proseReaderPath, setProseReaderPath] = useState<string | null>(null);

const handleFileSelect = (filePath: string) => {
  setShowFileBrowser(false);
  setProseReaderPath(filePath);
};
```

### Message Input Integration

The `onSendNotes` callback ultimately updates the message input's draft state. In the reference implementation, this uses a shared `diffCommentText` state that the MessageInput component watches via `useEffect` and auto-inserts.

For Phoenix UI, this should integrate with the existing `draft` state and `useDraft` hook, appending the formatted notes.

## File System API

The backend must provide two endpoints:

### List Directory Contents
```
GET /api/files/list?path={path}
```
Response: Array of file/directory metadata

### Read File Contents
```
GET /api/files/read?path={filePath}
```

Response: `{ content: string }`

If these endpoints don't exist, they need to be added as part of the implementation.

## Dependencies

- `react-markdown` - Markdown rendering
- `remark-gfm` - GitHub Flavored Markdown support  
- `react-syntax-highlighter` - Code syntax highlighting
- Prism styles for light/dark themes

## CSS Architecture

All styles namespaced to avoid conflicts:

### File Browser Classes
- `.file-browser-overlay` - Full-screen container
- `.file-browser-header` - Path display and navigation
- `.file-browser-list` - Scrollable file list
- `.file-browser-item` - Individual file/folder row
- `.file-browser-item--disabled` - Non-text file styling (grayed out)
- `.file-browser-empty` - Empty directory message

### Prose Reader Classes
- `.prose-reader-overlay` - Full-screen container
- `.prose-reader-header` - Top bar with back button, filename, notes badge
- `.prose-reader-content` - Scrollable content area
- `.prose-reader-line` - Selectable line wrapper with highlight animation
- `.prose-reader-annotation-overlay` - Dialog backdrop
- `.prose-reader-notes-panel` - Bottom drawer for notes list

## Testing Strategy

### Unit Tests

**File Browser:**
- File type detection from extension (including non-text files)
- Sorting logic (directories first, alphabetical)
- Path navigation (up/down directories)
- Human-readable file size formatting (KiB, MiB, GiB)
- Relative time formatting
- Text encoding detection (UTF-8, UTF-16, ASCII)
- Expanded state persistence and restoration

**Prose Reader:**
- Long-press timer logic (mocked timers)
- Movement threshold detection (10px cancellation)
- File type detection from extension
- Notes formatting function
- Long-press timer logic (mocked timers)

### Integration Tests

**File Browser Flow:**
- Open browser, navigate directories, select file
- Empty directory shows appropriate message  
- Long paths truncate correctly
- Up button disabled at root

**Prose Reader Flow:**  
- Render markdown file, verify formatted output
- Render code file, verify syntax highlighting applied
- Add note flow: long-press → dialog → add → verify in notes list
- Send notes: verify formatted output matches expected structure
- Close with unsaved notes: verify confirmation shown

**Patch Integration Flow:**
- Click file in patch output opens prose reader
- Modified lines show diff highlighting
- Auto-scroll to first modified line works
- Auto-prefix works for annotations on changed lines
- Banner shows patch context

### Manual Testing
- Mobile Safari: touch gestures, keyboard appearance, safe areas
- Desktop Chrome: mouse interactions, keyboard shortcuts
- Various file types and sizes
**Human-readable size formatting:**
```typescript
const formatFileSize = (bytes: number): string => {
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  let size = bytes;
  let unitIndex = 0;
  
  while (size >= 1024 && unitIndex < units.length - 1) {
    size /= 1024;
    unitIndex++;
  }
  
  return `${size.toFixed(1)} ${units[unitIndex]}`;
};
```**Icon Implementation:**
```typescript
// Use SVG icons or icon font (e.g., Feather Icons, Heroicons)
const FileIcon = ({ type }: { type: FileType }) => {
  const icons = {
    folder: <FolderIcon />,
    markdown: <FileTextIcon />,
    code: <CodeIcon />,
    config: <SettingsIcon />,
    text: <FileIcon />,
    image: <ImageIcon />,
    data: <DatabaseIcon />,
    unknown: <FileIcon />
  };
  
  return icons[type] || icons.unknown;
};
```

**Expanded State Persistence:**
```typescript
// Store in conversation-specific state or context
const [expandedPaths, setExpandedPaths] = useState<Set<string>>(() => {
  // Load from conversation context if available
  return new Set(conversationContext.expandedPaths || []);
});

// Save when paths change
useEffect(() => {
  conversationContext.setExpandedPaths(Array.from(expandedPaths));
}, [expandedPaths]);
```