# Add first-class commission review styling and result widgets

## Problem

Commission review is functionally wired into the conversation runtime, but its UI is unfinished in two visible places:

- `CommissionReviewApproval.tsx` uses nonexistent selectors (`task-approval-backdrop`, `task-approval-eyebrow`, `task-approval-section`, `task-approval-secondary`, and `task-approval-primary`) and an inline style. It therefore looks like an unstyled reuse of the task approval reader rather than a deliberate capital-spend decision surface.
- `commission_review` tool calls use the generic JSON input formatter, and completed results ignore the purpose-built `display_data.kind === "commission_review"` payload. The conversation renders a large raw JSON result instead of a review widget.

## Outcome

Deliver the viewer-independent frontend now: a polished approval experience and first-class inline commission-review call/result widgets with a fast deterministic visual QA loop. Defer only the full review viewer integration until PR 478 establishes the final conventions for non-file viewer-slot content.

## Implementation plan

### 1. Add a purpose-built commission review tool call/result renderer

- Define narrowly typed UI parsing/validation helpers for the existing commission-review input and `display_data` contract rather than scattering unchecked casts through the component.
- Render the request input as a concise capital-review card showing the executive brief and optional focus instead of raw JSON.
- Make the inline completed state a polished, information-dense review summary rather than raw JSON:
  - top-level completion/trust status and elapsed time;
  - finding counts by severity;
  - reviewer summary;
  - target and coverage statistics (base → head, files reviewed/changed, insertions/deletions);
  - prominent warnings and unreviewed-file coverage gaps;
  - a bounded preview of the highest-severity findings, including file/line/symbol when present;
  - clear intentional states for clean, findings-present, partial, failed, and rejected outcomes.
- Keep the component boundary ready for a future **Open full review** action, but do not add a disabled/dead control and do not modify `ViewerSlot` in this slice.
- Invest implementation effort in the real structured UI. Raw JSON is not normal UI. Keep only small defensive containment for malformed or historical payloads so one bad transcript row cannot crash rendering.
- Keep running, failed, missing-result, duration, and copy behavior consistent with the shared tool-card state machinery.

### 2. Restyle the approval view as an owned component

- Give `CommissionReviewApproval` an adjacent stylesheet and commission-specific class names; do not depend on accidental task-approval selectors or add more feature CSS to `index.css`.
- Build a full-height, responsive approval reader consistent with Phoenix's existing approval visual language while making the capital-spend context explicit.
- Present:
  - a compact title/status header;
  - the brief as the primary decision information;
  - optional focus;
  - a structured scope panel with repository, target kind, base → head, and diff stats;
  - an inline committed-only/clean-worktree policy cue;
  - clear success, rejection, and error states;
  - a sticky action bar with accessible reject and approve actions, busy states, and mobile-safe sizing.
- Remove inline styling and ensure long repository paths, refs, briefs, and focus text wrap without breaking the layout.
- Reuse theme tokens and established button/state colors; avoid introducing duplicate global primitives.

### 3. Add a dedicated `./dev.py qa commission-review` iteration loop

- Add deterministic scenario data under `ui/src/fixtures/commissionReview/`, a stable rendered-ready marker, Ladle stories, and a capture script using `runSurfaceCapture`.
- Register `qa:commission-review` in `ui/package.json` and `./dev.py qa commission-review` in the development CLI.
- Cover both desktop and mobile viewports and both light and dark themes. Scenarios must include:
  - approval with realistic scope and long repository/ref text;
  - approval without optional focus/scope;
  - inline request/running state;
  - clean completed review;
  - multiple findings with mixed severities;
  - partial review with warnings and unreviewed files;
  - failed and rejected outcomes.
- Keep scenarios timer-free and network-free; feed loaded structured payloads directly so screenshots are stable and fast.

### 4. Tests and validation

- Add component tests for the approval view: complete and absent scope, optional focus, approve/reject busy behavior, errors, disabled duplicate decisions, and accessible dialog/status semantics.
- Add `MessageComponents` tests for commission-review request formatting and inline structured summaries: clean success, findings across severities, partial coverage/unreviewed files, warnings/retry guidance, malformed display-data containment, and failed/rejected output.
- Add fixture/story integrity tests so every declared commission-review scenario is reachable and has deterministic data.
- Run the focused Vitest suites, TypeScript/lint checks, `./dev.py qa commission-review`, inspect the captures, and run `./dev.py check`.

## Acceptance criteria

- A `commission_review` call no longer displays its input as undifferentiated JSON.
- A valid completed commission review never presents raw JSON as normal UI; users can scan status, trust, coverage, summary, counts, and the most important findings inline.
- `./dev.py qa commission-review` captures deterministic approval and inline-result scenarios at desktop and mobile sizes for rapid visual iteration.
- Partial and incomplete reviews visibly distinguish warnings and unreviewed files from a clean review.
- Invalid or legacy payloads remain readable through a safe generic fallback and do not crash message rendering.
- The approval view is fully styled, responsive, keyboard-accessible, and uses no undefined commission-facing class selectors or inline presentation styles.
- Approval actions clearly communicate busy, settled, and error states and cannot be submitted twice.
- Automated tests cover both the approval surface and the structured tool result renderer.

## Scope constraints

- Do not modify `ViewerSlot`, add a full-review viewer, or couple this work to PR 478 while that PR is still under review. After it lands, follow-up work can add the full viewer using its finalized non-file-content conventions.
- Keep the existing backend review and approval contracts unless implementation discovers a concrete wire-shape defect required for correct rendering.
- Do not expose token or cost metadata; REQ-CR-014 explicitly excludes it from user-facing results.
- Do not weaken the committed-only, clean-worktree, or human-approval requirements in `specs/commission-review/requirements.md`.
- If spec status is touched, correct the stale executive summary that still describes the durable approval UI/state as incomplete, without adding rollout-relative text to timeless requirements.
