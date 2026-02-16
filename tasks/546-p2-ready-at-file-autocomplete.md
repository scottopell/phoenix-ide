---
created: 2026-02-15
priority: p2
status: ready
---

# @file Inline Autocomplete

## Summary

Implement `@file` mention syntax in the chat input with inline autocomplete, allowing users to quickly reference and include file contents in their messages.

## Context

Users often need to reference specific files when talking to the agent. Currently they must type full paths or rely on the agent to find files. An `@file` autocomplete would:

1. Let users type `@` to trigger file suggestions
2. Show matching files as they type (e.g., `@src/ma` shows `src/main.rs`)
3. Insert the file reference on selection
4. Optionally inline the file contents when the message is sent

### Design Considerations

**Single-tenant efficiency**: Phoenix IDE is single-user, single-conversation-active. We can:
- Keep a file index in memory (no need for per-request filesystem walks)
- Use the conversation's working directory as the search root
- Rebuild index on directory change or explicit refresh
- Consider `inotify`/`fswatch` for live updates (may be overkill)

**Autocomplete UX**:
- Trigger: `@` character in input field
- Filter: fuzzy match on path components (not just prefix)
- Display: show relative paths, highlight matched portions
- Limit: cap suggestions (20-50) for performance
- Escape hatch: `@@` to insert literal `@`

**File inclusion modes**:
- `@path/to/file` - reference only (agent sees path, can read if needed)
- `@path/to/file!` - inline contents (file content included in message)
- Could support `@path/to/file:10-20` for line ranges

**Index strategy**:
- Walk directory tree up to depth limit (e.g., 10 levels)
- Respect `.gitignore` patterns
- Skip common junk: `node_modules`, `.git`, `target`, `__pycache__`
- Store: path string + file type (for icons) + size (for inclusion warnings)
- Memory budget: ~100 bytes per file × 10k files = 1MB (acceptable)

**API design**:
- `GET /api/conversations/:id/files?q=<query>&limit=<n>` - search endpoint
- Or: WebSocket push of file index on conversation load
- Or: client-side index from initial file list (simplest)

## Acceptance Criteria

- [ ] Typing `@` in chat input opens autocomplete dropdown
- [ ] Autocomplete shows files matching typed text (fuzzy)
- [ ] Selecting a file inserts `@path/to/file` at cursor
- [ ] Works with conversation's working directory as root
- [ ] Respects `.gitignore` (doesn't suggest ignored files)
- [ ] Handles large directories gracefully (caps results, doesn't freeze UI)
- [ ] `@@` escapes to literal `@`
- [ ] Keyboard navigation: arrow keys, enter to select, escape to dismiss

## Stretch Goals

- [ ] `@file!` syntax to inline file contents
- [ ] `@file:10-20` for line ranges
- [ ] File type icons in autocomplete
- [ ] Recently-referenced files sorted to top
- [ ] `@dir/` to reference directory (agent gets file listing)

## Implementation Notes

### Backend

```rust
// New endpoint or extend existing
GET /api/conversations/:id/files?q=main&limit=20

// Response
{
  "files": [
    { "path": "src/main.rs", "type": "file", "size": 1234 },
    { "path": "src/main.ts", "type": "file", "size": 567 }
  ]
}
```

Index building:
- Use `ignore` crate (same as ripgrep) for gitignore-aware walking
- Build on conversation start, cache in `ConversationRuntime`
- Invalidate on working directory change

### Frontend

- Use existing textarea, add autocomplete overlay
- Libraries to consider: none needed if simple (just position a div)
- Track cursor position to place dropdown
- Debounce queries (100-200ms)

### Message Processing

When message contains `@path/to/file!`:
1. Read file contents server-side
2. Replace `@path!` with actual contents (or structured block)
3. Could use XML-style: `<file path="src/main.rs">contents</file>`

## Open Questions

1. Should autocomplete be client-side (send full file list) or server-side (query endpoint)?
   - Client-side simpler, fine for <10k files
   - Server-side scales better, allows smarter ranking

2. How to handle binary files in autocomplete?
   - Show but mark as binary, prevent `!` inclusion

3. Should `@file` references be resolved at send time or displayed to agent as-is?
   - Agent seeing `@src/main.rs` is useful context even without contents
   - Could do both: show path and contents
