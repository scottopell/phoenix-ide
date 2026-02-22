# File Explorer Panel - Executive Summary

## Requirements Summary

The File Explorer Panel provides persistent file browsing on desktop viewports. It adds a third column to the desktop layout, sitting between the conversation sidebar and main content area. Users see a tree view of the current conversation's working directory, with expandable folders and clickable files. Clicking a text file opens it in the prose reader, which renders in the main content area (replacing the conversation view temporarily). Both the sidebar and file explorer support independent collapse/expand, with the collapsed file explorer showing recent file icons for quick re-access. All panel states persist to localStorage. The feature builds on existing FileBrowser and ProseReader components, adapting them for persistent panel display rather than modal overlays. Mobile behavior remains unchanged.

## Technical Summary

Extends DesktopLayout to three columns with CSS flexbox. FileExplorerPanel component manages collapse state and renders either the full FileTree or a RecentFilesStrip. FileTree refactors logic from existing FileBrowser overlay, removing navigation chrome and adding active-file highlighting. Recent files tracked per-conversation in localStorage (max 5 files). FileExplorerContext provides communication between file tree and main content area for opening files in prose reader. ProseReader renders inline in main content area on desktop instead of as overlay. Panel widths are fixed (sidebar ~280px, file explorer ~250px) with main content taking remaining space (min 400px).

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-FE-001:** Three-Column Desktop Layout | ❌ Not Started | Extend DesktopLayout |
| **REQ-FE-002:** File Tree Display | ❌ Not Started | Refactor from FileBrowser |
| **REQ-FE-003:** File Selection | ❌ Not Started | Click to open in prose reader |
| **REQ-FE-004:** Panel Collapse - Expanded State | ❌ Not Started | Full tree with toggle |
| **REQ-FE-005:** Panel Collapse - Collapsed State | ❌ Not Started | Recent files strip |
| **REQ-FE-006:** Recent Files Tracking | ❌ Not Started | Per-conversation, localStorage |
| **REQ-FE-007:** Accordion Panel Behavior | ❌ Not Started | Independent collapse |
| **REQ-FE-008:** Prose Reader Integration | ❌ Not Started | Render in main content |
| **REQ-FE-009:** Visual Feedback | ❌ Not Started | Active file highlight |
| **REQ-FE-010:** Mobile File Browser Overlay | ❌ Not Started | Modal overlay hosting FileTree |

**Progress:** 0 of 10 complete
