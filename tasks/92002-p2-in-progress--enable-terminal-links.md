---
created: 2026-05-04
priority: p2
status: in-progress
artifact: pending
---

# enable-terminal-links

## Plan

## Summary
Enable clickable links in the embedded xterm terminal by default.

## Context
`ui/src/components/TerminalPanel.tsx` constructs an `@xterm/xterm` terminal and currently loads only `FitAddon`. xterm.js supports link detection via addons; the project does not currently depend on a link addon. The likely best path is `@xterm/addon-web-links`, which detects common URL text in the buffer and opens links through a handler. OSC 8 hyperlink support should be verified against the installed xterm version; if supported natively or via addon behavior, include it, otherwise capture a follow-up if deeper OSC 8 support requires custom parsing.

## What to do
1. Add the appropriate xterm link addon dependency, likely `@xterm/addon-web-links` matching the existing xterm 5.x package family.
2. Load the link addon in `TerminalPanel` during terminal initialization.
3. Configure activation behavior so links do not open accidentally during normal terminal use:
   - Prefer Cmd+click on macOS and Ctrl+click on non-macOS if the addon supports modifier gating.
   - If the addon cannot gate by modifier directly, use a custom handler/event gate or fall back to standard click with clear hover styling, documenting the limitation in code only where locally useful.
4. Open links safely with `window.open(url, '_blank', 'noopener,noreferrer')` or equivalent.
5. Verify whether OSC 8 hyperlinks render/click correctly in addition to raw URL text; if not feasible in this pass, leave normal URL autolinking implemented and note OSC 8 as a follow-up.
6. Add/update lightweight tests if the terminal setup is testable in the existing UI test stack; otherwise verify manually in dev.

## Acceptance criteria
- Plain URLs printed in the embedded terminal are visually recognized as links and can be opened from the terminal.
- The intended modifier interaction works: Cmd+click on macOS and/or Ctrl+click elsewhere, or a clearly documented fallback if xterm addon constraints prevent it.
- OSC 8 hyperlinks are either confirmed working or explicitly identified as not covered by the chosen addon.
- UI build/typecheck passes.
- If only UI files/package files change, Vite hot reload is sufficient; provide the dev URL for verification after implementation.

## Progress

