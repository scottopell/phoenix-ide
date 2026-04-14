---
created: 2026-04-14
priority: p3
status: ready
artifact: ui/src/index.css
---

# Terminal panel hardcodes dark colors, ignores `data-theme="light"`

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
