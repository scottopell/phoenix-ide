# Prose Feedback UI - Technical Design

## Architecture Overview

The Prose Feedback feature consists of a full-screen overlay component (`ProseReader`) that:
1. Fetches and renders text file content
2. Enables line-level annotation via long-press gesture
3. Manages a session-local collection of notes
4. Formats and injects notes into the message input

This is a **frontend-only feature** - no backend API changes required beyond the existing file read endpoint.

## Component Architecture

### REQ-PF-001 Implementation: ProseReader Component

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
- Text: all other extensions → monospace pre-formatted

### REQ-PF-002 Implementation: Long-Press Gesture

Gesture handling uses three touch event handlers per selectable element:
- `onTouchStart`: Start 500ms timer, store line info
- `onTouchMove`: Cancel timer (user is scrolling)
- `onTouchEnd`: Cancel timer if still running

Timer fires → trigger haptic feedback (`navigator.vibrate(50)`) and open annotation dialog.

For markdown content, each rendered block (p, h1-h3, li, blockquote) is wrapped with gesture handlers. A line counter increments during render to assign logical line numbers.

For code/text content, `react-syntax-highlighter`'s `lineProps` callback attaches handlers to each line, or lines are rendered individually in a loop.

### REQ-PF-003 Implementation: Annotation Dialog

**State:**
- `annotatingLine: { lineNumber: number; lineContent: string } | null`
- `noteInput: string`

Dialog is a bottom-sheet overlay (slides up from bottom). Auto-focuses textarea on open via `useEffect` with ref.

Keyboard shortcuts handled in textarea's `onKeyDown`:
- Escape → close dialog
- Ctrl/Cmd+Enter → submit note

### REQ-PF-004 Implementation: Notes Management

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

### REQ-PF-005 Implementation: Notes Formatting and Injection

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

### REQ-PF-006 Implementation: Close Confirmation

The `handleBack` function checks `notes.length > 0` before closing. Uses `window.confirm()` for simplicity, though a custom modal could be used for consistency.

### REQ-PF-007 Implementation: Session Scope

Notes state is local to the ProseReader component instance. When the component unmounts (on close), state is lost. No localStorage persistence is implemented - this is intentional per requirements.

### REQ-PF-008, REQ-PF-009 Implementation: Layout and States

CSS uses:
- `position: fixed; inset: 0` for full-screen overlay
- `height: 100dvh` for mobile viewport handling
- Bottom-sheet pattern with `animation: slide-up` for dialogs
- Loading/error states as centered flex containers

## Integration Points

### File Browser Integration

**Dependency**: This feature requires a file browser component. If one doesn't exist in the Phoenix UI, it needs to be specified and implemented first. The file browser should:
- Allow navigation through the project directory structure
- Support file selection with a callback
- Pass both the file path and root directory to consumers

The FileBrowser component calls `onFileSelect` with the file path. The parent (ChatInterface or ConversationPage) sets `proseReaderPath` state, which conditionally renders ProseReader.

### Message Input Integration

The `onSendNotes` callback ultimately updates the message input's draft state. In the reference implementation, this uses a shared `diffCommentText` state that the MessageInput component watches via `useEffect` and auto-inserts.

For Phoenix UI, this should integrate with the existing `draft` state and `useDraft` hook, appending the formatted notes.

## File Read API

The existing Phoenix backend must support reading text files. Expected endpoint:
```
GET /api/files/read?path={filePath}&root={rootDir}
```

Response: `{ content: string }`

If this endpoint doesn't exist, it needs to be added as a prerequisite.

## Dependencies

- `react-markdown` - Markdown rendering
- `remark-gfm` - GitHub Flavored Markdown support  
- `react-syntax-highlighter` - Code syntax highlighting
- Prism styles for light/dark themes

## CSS Architecture

All styles namespaced with `.prose-reader-*` prefix to avoid conflicts. Key classes:
- `.prose-reader-overlay` - Full-screen container
- `.prose-reader-header` - Top bar with back button, filename, notes badge
- `.prose-reader-content` - Scrollable content area
- `.prose-reader-line` - Selectable line wrapper with highlight animation
- `.prose-reader-annotation-overlay` - Dialog backdrop
- `.prose-reader-notes-panel` - Bottom drawer for notes list

## Testing Strategy

### Unit Tests
- File type detection from extension
- Notes formatting function
- Long-press timer logic (mocked timers)

### Integration Tests  
- Render markdown file, verify formatted output
- Render code file, verify syntax highlighting applied
- Add note flow: long-press → dialog → add → verify in notes list
- Send notes: verify formatted output matches expected structure
- Close with unsaved notes: verify confirmation shown

### Manual Testing
- Mobile Safari: touch gestures, keyboard appearance, safe areas
- Desktop Chrome: mouse interactions, keyboard shortcuts
- Various file types and sizes
