---
created: 2026-04-14
priority: p3
status: done
artifact: ui/src/index.css
---

# Terminal panel hardcodes dark colors, ignores `data-theme="light"`

## Resolution

Followed the recommended "follow theme" path so the terminal feels
consistent with the rest of the UI instead of being a permanent dark
island.

### CSS variables (`ui/src/index.css`)

Added `--terminal-bg`, `--terminal-header-bg`, `--terminal-border`,
`--terminal-fg`, `--terminal-cursor` to both `[data-theme="dark"]` and
`[data-theme="light"]`. The dark values match the previous hardcoded
hexes (`#1a1a1a`, `#252525`, `#333`); the light values use the existing
GitHub-light palette tones (`#f6f8fa`, `#eaeef2`, `#d0d7de`, `#1f2328`)
for consistency with the chat / sidebar / file tree.

`.terminal-panel` and `.terminal-panel-header` were updated to read the
new variables instead of literal hexes.

### xterm.js theme integration (`ui/src/components/TerminalPanel.tsx`)

xterm.js renders to its own canvas, so CSS doesn't reach
`.xterm-viewport`. The fix:

1. New helper `readXtermTheme()` reads `--terminal-bg/--terminal-fg/
   --terminal-cursor` from `:root` and returns an `ITheme`.
2. `new Terminal({ ... })` now uses `theme: readXtermTheme()` instead of
   the previous `{ background: '#1a1a1a', ... }` literal.
3. New `useEffect([theme])` (where `theme` is from `useTheme()`) reapplies
   the theme to the live `term.options.theme` whenever the app theme
   changes. This means the terminal will switch live without tearing
   down the PTY once a theme toggle button is wired up to the
   `useTheme().toggleTheme()` API. Today the only path is reload, which
   already worked via the construction-time read.

### Verification

DOM-level inspection in light mode:

```
panel_bg     = rgb(246, 248, 250)   ← #f6f8fa, the new --terminal-bg
viewport_bg  = rgb(246, 248, 250)   ← xterm.js canvas, matches
css_var      = #f6f8fa
```

After `localStorage.setItem('phoenix-theme','dark'); reload`:

```
panel_bg     = rgb(26, 26, 26)
viewport_bg  = rgb(26, 26, 26)
css_var      = #1a1a1a
```

Screenshot at `tasks/screenshots/17-terminal-light-mode.png` shows the
terminal blending into the light layout instead of the previous dark
strip.

### Out of scope

A user-facing theme toggle button. `ThemeToggle.tsx` exists but isn't
mounted anywhere — that's a separate UI wiring task. The
infrastructure to live-switch themes is now in place; only the trigger
is missing.

## Problem

`.terminal-panel` and `.terminal-panel-header` use literal hex values
rather than CSS variables, so they stay dark when the rest of the UI
switches to light mode:

```css
/* ui/src/index.css:6843 */
.terminal-panel {
  background: #1a1a1a;   /* hardcoded */
  ...
}
.terminal-panel-header {
  background: #252525;   /* hardcoded */
  border-bottom: 1px solid #333;   /* hardcoded */
  ...
}
```

xterm.js's `Terminal` is instantiated without a `theme:` override
(`ui/src/components/TerminalPanel.tsx:282+`), so `.xterm-viewport` also
stays dark.

## How this was found

While switching to light mode via `data-theme="light"` on `<html>` and
auditing computed backgrounds, these are the only opaque dark elements
left in the layout. Every other panel (sidebar, file explorer, chat
column, message input) correctly picks up the light palette via
existing CSS variables — so this is one outlier, not a sweeping
theming gap.

```js
// Hunt for opaque dark backgrounds while in light mode
for (const el of document.querySelectorAll("*")) {
  const bg = getComputedStyle(el).backgroundColor;
  // ... filter for rgba with alpha > 0.5 and r,g,b all < 80
}
// → [.terminal-panel, .xterm-viewport]
```

## Open question

Should the terminal follow the app theme, or intentionally stay dark
(like VS Code's integrated terminal, which has its own theme separate
from the editor theme)?

- **Always dark**: common convention for integrated terminals; avoids a
  jarring bright terminal in an otherwise light doc-reading flow.
  Cheap to keep — just document it and update the light-mode QA
  checklist to expect dark terminal.
- **Follow theme**: consistency with the rest of the app; requires
  introducing CSS variables for terminal chrome colors and a matching
  xterm.js theme object (`new Terminal({ theme: { background, ... } })`)
  derived from the resolved theme.

Recommendation: give the user the choice. Default to "follow theme"
because Phoenix's design philosophy (AGENTS.md → UI Design Philosophy)
is "professional tool" and consistent theming is the less-surprising
default; add a preference toggle if users complain.

## Fix sketch

1. Introduce `--terminal-bg`, `--terminal-header-bg`, `--terminal-border`
   CSS variables alongside the existing theme variables in `:root[data-theme=...]`.
2. Replace the literals in `.terminal-panel*` with `var(--terminal-bg)` etc.
3. In `TerminalPanel.tsx`, read the variables at xterm construction time
   (or on theme change) and pass a `theme:` object to `new Terminal({..})`
   so `.xterm-viewport` matches.
4. Add a `useTheme()` effect that reapplies `xterm.options.theme` when
   the theme changes, without tearing down the PTY.

## Done when

- [ ] `.terminal-panel` uses CSS variables for backgrounds/borders
- [ ] xterm.js `theme` is derived from the current app theme
- [ ] Switching `data-theme` live updates the terminal without a reload
- [ ] Explicit product decision recorded (always-dark vs follow-theme)
- [ ] If follow-theme: the light-mode screenshot no longer has any
      opaque element with `r,g,b < 80`
