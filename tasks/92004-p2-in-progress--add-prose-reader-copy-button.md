---
created: 2026-05-09
priority: p2
status: in-progress
artifact: pending
---

# add-prose-reader-copy-button

## Plan

## Summary
Add a small top-right copy-to-clipboard action to the mobile prose/file reader header, implemented by reasoning over the existing React/CSS only. I will not install/hydrate node modules or run the live server.

## Context
`ProseReader` already owns the raw file contents in its `content` state and renders through `ViewerShell`, whose header has a right-side `headerExtras` slot. Clipboard support already exists in `ui/src/utils/clipboard.ts`, including a fallback for contexts where `navigator.clipboard` is unavailable.

## What I’ll do
1. Update `ui/src/components/ProseReader.tsx`:
   - Import `copyToClipboard`.
   - Add small hand-rolled inline SVG icon components for copy / copied states, matching the existing preference for simple manual icons instead of adding more lucide icons.
   - Add a small `copied` state with a short reset timer.
   - Add a `handleCopyFile` callback that copies the raw `content` string, not the rendered/selected markdown text.
   - Render a header action via `headerExtras`, disabled until content is available.
   - Preserve the existing HTML preview/source actions by composing them after the copy button.
2. Update `ui/src/index.css`:
   - Add compact styling for the new viewer copy button in the existing `.viewer-shell-actions` area.
   - Include hover/disabled/copied states using existing theme variables.
   - Keep it small but touch-friendly enough for mobile.
3. Validation by inspection only:
   - No `npm`, no node hydration, no live server.
   - I’ll sanity-check TypeScript/JSX structure by reading the surrounding code and ensuring imports/hooks/dependencies are consistent.

## Acceptance criteria
- In the mobile prose reader header, a copy button appears at the top right next to any existing header actions.
- Tapping it copies the full raw file contents to clipboard.
- The button gives brief visual feedback after a successful copy.
- The button is disabled while the file is loading or failed to load.
- Existing close, annotation, notes, send, and HTML preview/source behavior remains unchanged.
- No server or Node-based UI commands are run during implementation.

## Progress

