# Fix manual refresh for expanded file-tree directories

## Problem

The grounding panel's refresh button increments `refreshKey`, but `FileTree` only uses that key in the root-directory loading effect. Entries for expanded directories remain in `childItems`, so files added, removed, renamed, or modified below the root stay stale after an explicit refresh. This makes the refresh control appear broken during normal navigation.

The `/api/files/list` endpoint reads the filesystem directly; the stale result is owned by frontend state rather than backend caching.

## Plan

1. Define explicit refresh behavior in the file-explorer requirement: refreshing the tree reloads the root and every currently expanded directory while preserving expansion state.
2. Update `FileTree` so a changed manual-refresh signal invalidates/reloads all visible expanded directory listings, not only the root listing.
3. Keep refresh lifecycle safe across rapid repeated clicks, path/conversation changes, and component unmounts so an older request cannot restore stale child data.
4. Add focused UI regression coverage that mutates the mocked contents of an expanded directory, triggers refresh through the grounding-panel control, and verifies the visible nested rows update while the directory remains expanded.
5. Run the focused Vitest suite and the project check lanes applicable to the UI/spec changes.

## Acceptance criteria

- Clicking **Refresh file tree** reflects filesystem changes at the tree root and inside every expanded directory.
- Expanded/collapsed state is preserved.
- Collapsed directories are not fetched unnecessarily.
- Repeated refreshes and unmount/path changes do not commit stale responses or leak work.
- Automated coverage exercises the real `FileExplorerPanel` button-to-`FileTree` refresh path.
