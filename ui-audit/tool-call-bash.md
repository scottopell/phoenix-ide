# Widget: tool-call-bash

**Component:** `ui/src/components/MessageComponents.tsx:87-94, 377-534` **Type:** Tool
call **Description:** bash tool — $command display with multiline flag, copy button,
collapsible output with line counts, success/error status badge.

## Screenshots

Stored at `ui-audit/screenshots/tool-call-bash/`.

- [x] `01-success.png` — isolated success state
- [x] `02-alt.png` — non-happy-path state (error / truncated / loading — mark n/a if no
  material alt)
- [x] `03-in-context.png` — widget in a real conversation stack (5–10 surrounding
  widgets)

## Scores

| Dimension | Score (1–5) | Notes |
| --- | --- | --- |
| Information density | 4 | `$ ` prefix + inline `✓` in top-right + line-count on collapsed output packs shell semantics per row; “PHOENIX 10:22 AM” timestamp row above every tool call is the one recurring waste. |
| State legibility | 4 | `✓` / `✗` icon top-right is clear; failure in `02-alt.png` (`grep -n "fooBs" ... \| head -10` with `(empty)`) reads correctly but relies almost entirely on the small corner badge — error output is not tinted and the output area is not framed in red. |
| Consistency with shared widget grammar | 5 | Uses the same header/input/output scaffolding as every other tool-block; the `$ ` prefix is a meaningful specialization, not a layout divergence. |
| Scannability in context | 4 | In `03-in-context.png` the `$ ...` sigil reads as shell at a glance and sits visually distinct from `read_file`/`think`; the tool name label is small but legible. Long stdout pushes successor widgets far down — still scannable because of sigil. |
| Fidelity | 4 | Long output truncation is honest (`... (N more chars)`), `lineCount` shown on collapsed header, `(empty)` placeholder for zero-byte results. One gap: the `(empty)` result in `02-alt.png` still shows a `✓`, so a command that failed via “no matching output” looks identical to a successful no-op. |
| **Total** | **21 / 25** |  |

## Issues

- [density] Every tool-block is preceded by a full “PHOENIX 10:22 AM” agent-message
  header row; a stack of tool calls pays that row tax even though they all belong to the
  same turn (see `03-in-context.png`).
- [state-legibility] Error state relies on a 14px `✗` in the header — the output pane
  itself has a CSS `error` class but in `02-alt.png` the body does not visually differ
  enough from success at arm’s length.
- [state-legibility] Empty-output success (`(empty)` with `✓`) is indistinguishable from
  a grep that found nothing (arguably a failure in intent); source shows `isError` is
  purely `is_error || error`, so stderr-silent exit-1 can still render as success if the
  backend doesn’t set the flag.
- [fidelity] `displayResult` truncation at 5000 chars is fine, but the preview path
  (collapsed long output) only ever shows `previewLines[0..3]` + “+N more lines” — the
  last line of a failing command (often the actual error) is never the one previewed.
- [density] The output-header row (`chevron` + `output` label + `N lines` + copy) spends
  6 horizontal elements saying “click to collapse”; the `output` word is pure chrome.

## Recommendations

-----
Audited by: Scott
Notes: recommendations are solid, collapsing per-tool timestamp header is a
solid recc reaching beyond this individual widget. useful feedback.

Also reccommmend slightly decreasing the vertical padding around the tool call
name "bash" in the header row, also potentially across widgets.
-----

- Tail-bias the collapsed preview: show the last 3 lines (or first-1 + last-2) instead
  of the first 3, since bash failures surface at the tail.
- Drop the literal word `output` from the expanded output header row — the chevron +
  line count already signal it.
  Saves a column and reduces label noise when 10 bash calls stack.
- Tint the output pane background (not just the corner icon) when `isError` is true, so
  failure is readable from the scroll density without hunting for the 14px `✗`.
- Collapse the per-tool “PHOENIX HH:MM” header when the previous visible element is also
  a Phoenix tool_use within the same agent message — it’s duplicated 3× in
  `01-success.png` alone.
- When a bash result is `(empty)` AND `is_error` is false, show a neutral glyph (e.g.
  `·`) instead of `✓` — a silent exit-0 with no stdout is not the same signal as a
  successful command that produced output.
