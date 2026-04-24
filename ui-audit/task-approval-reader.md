# Widget: task-approval-reader

**Component:** `ui/src/components/TaskApprovalReader.tsx:179-582` **Type:** Inline
approval **Description:** Modal plan-approval UI — annotatable markdown, long-press
annotation, notes panel, Discard/Feedback/Approve actions; not dismissable via Escape.

**Live-interaction pair:** Shown during `awaiting_task_approval`; the history view after
approval/discard/feedback falls through `tool-call-generic`. Low-frequency widget —
appears once per task approval, rare vs.
tools like bash/patch.

## Screenshots

Stored at `ui-audit/screenshots/task-approval-reader/`.

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
