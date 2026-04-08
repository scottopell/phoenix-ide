---
created: 2026-04-08
priority: p2
status: in-progress
artifact: pending
---

# git-status-file-sidebar

## Plan

## Summary

Add git file status indicators to the file explorer sidebar, showing modified (M), added (A), untracked (U), deleted (D), and renamed (R) files with color-coded letters — matching the information-dense, symbol-driven UI philosophy.

## Context

- The file tree sidebar (`FileExplorerPanel`) is **only rendered on desktop** (≥1025px via `DesktopLayout`). On mobile, `FileBrowserOverlay` only opens on explicit user action. This means no extra work happens on mobile for the periodic refresh.
- The existing `/api/files/list` endpoint already does gitignore detection, so adding git status is a natural extension.
- The backend already has `detect_git_repo_root()` in `src/db/schema.rs` and git command infrastructure.

## Plan

### Backend (`src/api/types.rs` + `src/api/handlers.rs`)

1. **Add `git_status` field to `FileEntry`**:
   ```rust
   #[serde(skip_serializing_if = "Option::is_none")]
   pub git_status: Option<String>, // "M", "A", "D", "R", "U", "?" or None
   ```

2. **Compute git status in `list_files` handler**:
   - Detect git repo root for the listed directory (reuse `detect_git_repo_root`)
   - Run `git status --porcelain=v1 -z` from the repo root (null-delimited for safe parsing)
   - Parse output into a `HashMap<PathBuf, char>` mapping file paths → status
   - For each `FileEntry`:
     - **Files**: Direct lookup by path relative to repo root
     - **Directories**: Check if any status entry is a descendant; use the most severe status (priority: conflict > deleted > modified > added > untracked)
   - If not in a git repo, all entries get `git_status: None`

### Frontend (`ui/src/components/FileExplorer/FileTree.tsx` + `ui/src/index.css`)

3. **Add `git_status` to `FileItem` interface**:
   ```typescript
   git_status?: string | null; // "M", "A", "D", "R", "U", "?"
   ```

4. **Render status indicator** in `renderItem`, after the filename:
   ```tsx
   {item.git_status && (
     <span className={`ft-git-status ft-git-status--${gitStatusClass(item.git_status)}`}>
       {item.git_status === '?' ? 'U' : item.git_status}
     </span>
   )}
   ```

5. **CSS**: Right-aligned, small, monospace single letter:
   - `M` (modified) → orange/amber (`var(--accent-orange)`)
   - `A` (added/staged) → green (`var(--accent-green)`)
   - `U` / `?` (untracked) → green, slightly muted
   - `D` (deleted) → red
   - `R` (renamed) → blue
   - `C` (conflict) → red, bold
   - Font size ~10px, flex-shrink: 0, positioned at the end of the row

## What This Does NOT Do

- No separate API endpoint — status piggybacked on existing `/api/files/list`
- No new polling — rides the existing ~10s auto-refresh in `FileTree`
- No mobile-specific work — `FileExplorerPanel` already isn't rendered on mobile
- No staged vs unstaged distinction (v1 simplification — show most significant status)

## Acceptance Criteria

- [ ] Files with uncommitted changes show a colored status letter (M/A/D/R/U) in the file tree
- [ ] Directories containing changed files show the most severe child status
- [ ] Non-git directories work unchanged (no status shown)
- [ ] No extra API calls — status is part of the existing file list response
- [ ] `./dev.py check` passes (clippy, fmt, tests)
- [ ] Visual: indicators are subtle, right-aligned, don't crowd the filename


## Progress

