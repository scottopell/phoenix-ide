# Visually distinguish workscope internal groups from grounding panel separators

## Goal

In the grounding pane, the WORK section currently separates its internal resource groups (`bash`, `tmux`, `browser`) with the same full-width, high-contrast horizontal rules used between top-level grounding sections. This makes intra-component boundaries look like inter-component boundaries.

Update the workscope panel visuals so users can quickly distinguish:

- **Top-level grounding boundaries**: strong, full-width separators between FILES / MCP / SKILLS / TASKS / WORK.
- **Workscope internal grouping**: softer, inset, or card-like grouping between bash / tmux / browser.

## Proposed approach

1. Adjust `ui/src/components/WorkScopePanel.css` only unless a fixture/test reveals a structural need.
2. Replace `.ws-section { border-bottom: 1px solid var(--border-color); }` in the left grounding-panel context with a visually distinct internal treatment, for example:
   - no full-width divider;
   - subtle inset separator (`margin-left` aligned after the WORK icon/title gutter), or
   - soft section background / spacing / left accent that reads as grouping rather than panel boundary.
3. Preserve density: avoid adding large vertical whitespace or heavy cards.
4. Ensure the standalone chain-page workscope dock still looks coherent; either share the improved internal style or scope any differences intentionally.
5. Validate using the grounding panel fixture / screenshot path and at least a focused UI test if existing snapshots or DOM tests cover the panel.

## Acceptance criteria

- Bash/tmux/browser boundaries inside WORK no longer look like the top-level grounding section separators.
- The WORK section remains compact and scannable.
- The visual hierarchy is clear in both light/dark themes if both are supported by the fixture.
- No behavior changes to inventory polling, row expansion, or inspect/open affordances.
