# Widget: question-panel

**Component:** `ui/src/components/QuestionPanel.tsx:44-1000` **Type:** Inline approval
**Description:** Step-by-step question wizard — breadcrumb nav, single/multi-select with
preview pane, optional “Other” text input, notes expansion, Decline/Back/Next/Submit,
full keyboard nav.

**Live-interaction pair:** Shown when the agent awaits a user response via
`ask_user_question`; the history view falls through `tool-call-generic`. Low-frequency
widget — appears rarely vs.
tools like bash/patch.

## Screenshots

Stored at `ui-audit/screenshots/question-panel/`.

- [ ] `01-success.png` — isolated success state
- [ ] `02-alt.png` — non-happy-path state (error / truncated / loading — mark n/a if no
  material alt)
- [ ] `03-in-context.png` — widget in a real conversation stack (5–10 surrounding
  widgets)

## Scores

| Dimension | Score (1–5) | Notes |
| --- | --- | --- |
| Information density | _ |  |
| State legibility | _ |  |
| Consistency with shared widget grammar | _ |  |
| Scannability in context | _ |  |
| Fidelity | _ |  |
| **Total** | **_ / 25** |  |

## Issues

*Stage 2 — fill in specific findings per dimension.
Cite the element, not the aspiration.*

## Recommendations

*Stage 2 — what to change, in priority order.*
