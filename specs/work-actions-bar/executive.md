# Work Actions Bar — Executive Summary

## What This Spec Covers

The work actions surface is rendered in Work and Branch conversations while the conversation is
idle, errored, or context-exhausted. Desktop presents a `.work-actions-bar` row; narrow mobile
viewports preserve the transcript by presenting one contextual primary and disclosing the complete
action set in a viewport-bound dialog. Its purpose is first-time legibility: a user should know
the next action immediately and be able to inspect every other available verb without losing the
conversation as the primary work surface.

This is a composition spec. It selects which verb is primary and which verbs are present; it
re-derives none of its inputs. It owns:

- **Visibility** — when the bar appears and when it is hidden.
- **Responsive three-zone presentation** — REVIEW, RESOLVE, and FINISH, the single-primary rule,
  and the mobile persistent-primary plus dialog presentation.
- **`WorkDisposition` derivation** — the single derived state that selects the primary verb,
  total over every open-PR and stuck-with-PR case.
- **Verb labels and tooltip copy** — the text of each button, info-icon tooltip, and inline
  note.
- **Structural interaction principles** — no disabled buttons used as status displays; no
  click-to-enable-then-click-again toggles.
- **View Browser exclusion** — why the browser session affordance lives in the work scope,
  surfaced through the viewer slot, not in this bar.

It does **not** own:

- **PR status, explicit active-PR selection, any compatibility primary-PR projection, the CI
  check-state and feedback-freshness signals, or the auto-fix affordance** — the
  `pr-association` spec (`PrStatusView`, `PrCheckState`, `PrFeedbackFreshness`,
  `PrAutoFixAffordance`, `WorkScopePrStatusContract`, `WorkActionsPrAffordanceContract`).
- **Terminal action git semantics** — worktree deletion, diff snapshot, confirmation gate,
  mode-dependent branch disposition — the `work-lifecycle` spec (REQ-WL-001/002/003).
- **Action legality** — when a terminal action may legally fire — bedrock's `TaskResolved`
  rule (REQ-BED-029 terminal-on-resolution; REQ-BED-031 context-exhausted disposability).
- **Diff and browser viewer mechanics** — the `viewer_slot` spec (REQ-VS-003 diff; REQ-VS-008
  browser), and the browser session inventory in `work-scope-ui` (REQ-WSUI-004).

## User Need

A developer who has finished (or got stuck on) a Work or Branch conversation needs to know, at
a glance, what to do next. Status is shown in the StateBar (the PR badge). The work actions bar
is pure verbs: each button does one fixed thing when clicked, present only when that thing is
safe to do. No button is disabled and used as a status display; no button requires a second
click to arm.

## Requirements Summary

| ID | Summary |
|----|---------|
| REQ-WAB-001 | Bar visibility: Work/Branch mode AND phase ∈ {idle, error, context_exhausted} |
| REQ-WAB-002 | Responsive three-zone presentation: desktop row; mobile persistent primary plus complete dialog |
| REQ-WAB-003 | Exactly one primary (glowing) verb across the bar — or none, in the continuation case |
| REQ-WAB-004 | WorkDisposition derivation: a single derived state, total over every open-PR and stuck-with-PR case |
| REQ-WAB-005 | RESOLVE zone suppressed in stuck phases (error, context_exhausted) |
| REQ-WAB-006 | View Browser is not in this bar; the browser session affordance belongs to the work scope, surfaced via the viewer slot |
| REQ-WAB-007 | Clean up and Abandon: info-icon tooltips explain intent and the diff-snapshot/confirm difference, mode-sensitive |
| REQ-WAB-008 | No disabled-as-status buttons; no two-step toggle affordances |
| REQ-WAB-009 | Continuation mute: when continued_in_conv_id is set, RESOLVE and FINISH are suppressed and there is no primary |
| REQ-WAB-010 | PR link verbs (Merge / Open PR) are GitHub links; Phoenix has no merge API |
| REQ-WAB-011 | Mobile dialog is viewport-safe, focus-managed, touch-safe, structured by zone, and shares active-PR authority |

Increment 1 also depends on the `pr-association` migration from hidden singular-primary targeting
to one explicit active PR. The work actions bar remains a composition surface: it does not infer
among multiple associated PRs itself. Its job is to render whatever explicit active selection the
PR-association layer provides, and to stay silent rather than silently retarget when selection is
ambiguous.

## Implementation Status

| ID | Status |
|----|--------|
| REQ-WAB-001 | Implemented |
| REQ-WAB-002 | Implemented |
| REQ-WAB-003 | Implemented |
| REQ-WAB-004 | Implemented |
| REQ-WAB-005 | Implemented |
| REQ-WAB-006 | Implemented |
| REQ-WAB-007 | Implemented |
| REQ-WAB-008 | Implemented |
| REQ-WAB-009 | Implemented |
| REQ-WAB-010 | Implemented |
| REQ-WAB-011 | Implemented |

## Implementation Map

| Surface | Location |
|---------|----------|
| `WorkControlBar` component | `ui/src/components/WorkActions.tsx` |
| Mobile work-action dialog styling | `ui/src/components/WorkActions.css` |
| `WorkDisposition` derivation | `deriveWorkDisposition` in the same file |
| Shared FINISH-primary selector | `finishPrimaryForDisposition` in the same file |
| PR status polling | `ui/src/hooks/useConversationPrStatus.ts` |
| PR feedback freshness label | `ui/src/components/prBadge.ts` (`prFeedbackFreshnessLabel`) |
