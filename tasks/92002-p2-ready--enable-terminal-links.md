---
created: 2026-05-04
priority: p2
status: ready
artifact: ui/src/components/TerminalPanel.tsx
---

# enable-terminal-links

## Summary
Enable clickable links in the embedded xterm terminal by default so URLs printed by command output can be opened directly from the terminal.

## Context
`ui/src/components/TerminalPanel.tsx` constructs an `@xterm/xterm` terminal and currently loads only `FitAddon`. The UI dependencies include `@xterm/xterm` and `@xterm/addon-fit`, but not a link addon. xterm.js 5.x offers link support through `@xterm/addon-web-links` for raw URL text in the terminal buffer, and xterm core has link/provider APIs that may also support OSC 8 hyperlinks depending on version and configuration.

This worktree/environment currently lacks the Node/npm toolchain, so implementation should happen in an environment where `npm install`, `npm run build`, and the UI dev server are available.

## Desired behavior
- Plain URL text printed in terminal output is visually recognized as a link.
- Opening a link should require an intentional interaction, ideally:
  - macOS: Cmd+click
  - other platforms: Ctrl+click
- Links should open in a new tab/window using a safe opener pattern such as `window.open(url, '_blank', 'noopener,noreferrer')`.
- Normal terminal selection, focus, keyboard input, and copy/paste behavior should not regress.

## Implementation notes
1. Add an xterm link dependency, likely `@xterm/addon-web-links` at a version compatible with the existing xterm 5.x packages.
2. Import and load the web links addon alongside `FitAddon` in `TerminalPanel` initialization.
3. Check the installed addon typings/API for modifier gating support:
   - If `WebLinksAddon` supports a validation/activation callback with mouse event details, enforce Cmd/Ctrl there.
   - If it does not directly support modifier gating, investigate xterm's link provider APIs or a minimal custom link provider.
   - If modifier gating is not practical in this pass, implement normal click-to-open and document the limitation in this task/PR rather than in broad code comments.
4. Ensure URL opening uses a safe handler and does not rely on default `window.location` navigation.
5. Verify OSC 8 behavior explicitly with a command like:
   ```bash
   printf '\e]8;;https://example.com\aExample link\e]8;;\a\n'
   ```
   If OSC 8 is not covered by `@xterm/addon-web-links`, either support it through xterm's native link APIs or file/keep a follow-up note.
6. Add lightweight tests only if the terminal setup is practical to exercise under the existing UI test stack; otherwise document manual verification steps in the PR.

## Acceptance criteria
- Plain URLs printed in the embedded terminal are visually recognized as links and can be opened.
- The intended modifier interaction works (Cmd+click on macOS and/or Ctrl+click elsewhere), or the PR clearly states the chosen fallback and why.
- OSC 8 hyperlinks are confirmed working or explicitly identified as not covered by the chosen implementation.
- `npm run build` passes in `ui/`.
- If only UI/package files change, Vite hot reload is sufficient; provide the dev URL for verification after implementation.

## Manual verification checklist
- Start Phoenix with `./dev.py up`.
- Open a conversation with the terminal visible.
- Run:
  ```bash
  printf 'raw url: https://example.com\n'
  printf '\e]8;;https://example.com\aosc8 example\e]8;;\a\n'
  ```
- Verify raw URL link styling/hover behavior.
- Verify the expected modifier-click opens `https://example.com` in a new tab.
- Verify non-modified click does not unexpectedly navigate if modifier gating was implemented.
- Verify text selection and normal terminal input still work.

## Progress
- Task captured and fleshed out.
- Initial environment check found `npm` unavailable in this worktree session, so dependency installation/build verification is deferred to a web-dev-capable environment.
