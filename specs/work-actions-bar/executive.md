# Work Actions Bar — Executive Summary

## What This Spec Covers

The work actions bar describes the current finish/review/resolve action rail shown for legacy Work and Branch conversations, including PR-driven action emphasis, tooltip copy, and mobile/desktop presentation.

## Current Reality

This surface is implemented against the pre-unification product model. It still appears only for legacy Work/Branch contexts, still exposes legacy FINISH actions such as **Clean up** / **Abandon** semantics tied to current PR and branch state, and still composes around the shipped mark-merged / abandon flows rather than a unified Close conversation action. The bar therefore remains implemented current reality, but not current normative lifecycle design.

Anchors for that current reality include `ui/src/components/WorkActions.tsx`, `ui/src/components/prRailAvailability.ts`, `ui/src/components/prReviewState.ts`, and `ui/src/hooks/useConversationPrStatus.ts`.

## Requirements Summary

| ID | Summary |
|----|---------|
| REQ-WAB-001 | Bar visibility: Work/Branch mode AND phase ∈ {idle, error, recoverable_continuation_failure} |
| REQ-WAB-002 | Responsive presentation: stable compact desktop rail; mobile PR rail; hero and supporting action groups; freshness stays an actionability cue, not PR status |
| REQ-WAB-003 | Exactly one primary (glowing) verb across the bar — or none, in the continuation case |
| REQ-WAB-004 | WorkDisposition derivation: a single derived state, total over every open-PR and stuck-with-PR case |
| REQ-WAB-005 | RESOLVE zone suppressed in stuck phases (`error`, `recoverable_continuation_failure`) |
| REQ-WAB-006 | View Browser is not in this bar; the browser session affordance belongs to the work scope, surfaced via the viewer slot |
| REQ-WAB-007 | Clean up and Abandon: info-icon tooltips explain intent and the diff-snapshot/confirm difference, mode-sensitive |
| REQ-WAB-008 | No disabled-as-status buttons; StateBar owns stable PR identity/status; no two-step toggle affordances |
| REQ-WAB-009 | Continuation mute: when continued_in_conv_id is set, RESOLVE and FINISH are suppressed and there is no primary |
| REQ-WAB-010 | PR link verbs (Merge / Open PR) are GitHub links; Phoenix has no merge API |
| REQ-WAB-011 | Mobile rail shows actionable PR status/freshness and expands one active PR without parallel selection state |
| REQ-WAB-012 | Desktop multi-PR rail shows rich PR context and sidebar-consistent review state, shares active selection authority, and preserves selector fallback |

## Implementation Status

| ID | Status | Notes |
|----|--------|-------|
| REQ-WAB-001 | Implemented (legacy current reality) | Still keyed to Work/Branch mode visibility |
| REQ-WAB-002 | Implemented (legacy current reality) | Current responsive rail is shipped |
| REQ-WAB-003 | Implemented (legacy current reality) | Current single-primary derivation is shipped |
| REQ-WAB-004 | Implemented (legacy current reality) | `WorkDisposition` remains derived from current PR/work state |
| REQ-WAB-005 | Implemented (legacy current reality) | Stuck-phase suppression is shipped |
| REQ-WAB-006 | Implemented | Browser affordance remains outside this bar |
| REQ-WAB-007 | Implemented (legacy current reality) | Tooltips still explain legacy Clean up / Abandon semantics |
| REQ-WAB-008 | Implemented | No disabled-as-status behavior in shipped rail |
| REQ-WAB-009 | Implemented | Continuation still suppresses legacy finish actions on predecessors |
| REQ-WAB-010 | Implemented | GitHub link behavior remains shipped |
| REQ-WAB-011 | Implemented | Mobile rail behavior is shipped |
| REQ-WAB-012 | Implemented | Desktop multi-PR rail behavior is shipped |

## Reconciliation Note

When the unified lifecycle lands, this spec's current implementation status will need to be revisited around visibility, primary verb selection, and the removal of Clean up / Abandon terminology. For now, the honest status is: implemented, but implemented against the legacy lifecycle surface.
