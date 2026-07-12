# Phoenix Coordinator — Executive Summary

## Requirements Summary

Phoenix Coordinator gives users a deliberate Phoenix-wide surface for
orientation and cross-conversation analysis. The fleet view answers "what
active work exists?" deterministically by grouping visible work by project and
representing each open work item as either a continuation chain or standalone
conversation. Each item carries metadata and explainable signals so the user can
see why it appears.

The Coordinator itself is one durable conversation identity that uses the normal
Phoenix conversation runtime and UI. It reuses the standard transcript,
composer, streaming, persistence, and continuation behavior while being framed
as a global read-only coordinator rather than ordinary project work. The
Coordinator surface keeps the compact fleet snapshot visible so the user can
triage and inspect work without leaving the page.

## Technical Summary

The fleet projection derives from persisted conversations, project rows,
continuation links, conversation modes, runtime states, and task metadata when
available. Continuation topology is reconstructed before visibility filtering,
so archived historical members do not change a chain's durable root. Inclusion
requires positive evidence: open task status or an active/attention state for
Work mode, and active/attention state or activity within 14 days for Direct,
Explore, and Branch modes.

The UI now consumes `/api/global/coordinator`, which resolves the singleton
Coordinator conversation and lets the page compose that identity with the
existing deterministic fleet data from `/api/global/open-work`. The bespoke
Global Recall session API/types/UI are retired from the frontend contract. The
Coordinator keeps using app-local links, durable references, and bounded global
read tooling for synthesis and citation workflows.

## Status Summary

| Requirement | Status | Notes |
|---|---|---|
| **REQ-GR-001:** View Active Work Without Model Inference | ✅ Complete | `/api/global/open-work` builds a deterministic projection; `/global` renders it without model calls |
| **REQ-GR-002:** Collapse Continuation Chains Into One Work Item | ✅ Complete | Projection reconstructs durable `continued_in_conv_id` topology before visibility filtering |
| **REQ-GR-003:** Explain Why Work Appears | ✅ Complete | Positive-evidence inclusion and closed-state suppression are covered by projection tests; items expose recency, mode, state, task, and chain signals |
| **REQ-GR-004:** Surface Work Identity Metadata | ✅ Complete | Fleet items expose mode/task/branch/worktree/current/root/update metadata when present |
| **REQ-GR-005:** Provide Stable References and App-Local Links | ✅ Complete | Open-work rows expose durable `@work:` handles; resolution reconstructs historical source/status after closure |
| **REQ-GR-006:** Provide One Durable Coordinator Identity | ✅ Complete | `/api/global/coordinator` is the frontend contract for resolving the singleton Coordinator conversation |
| **REQ-GR-007:** Restrict Phoenix-wide Tools to the Coordinator | ✅ Complete | Global search/read/open-work/reference tools remain reserved for the Coordinator flow |
| **REQ-GR-008:** Answer With Source Citations | ✅ Complete | Coordinator synthesis is expected to cite app-local conversation/message sources and stable handles |
| **REQ-GR-009:** Resolve Copied References | ✅ Complete | `/api/global/resolve` supports typed handles and app-local conversation/chain paths |
| **REQ-GR-010:** Keep the Fleet Snapshot Visible on the Coordinator Surface | ✅ Complete | `/global` remains a Coordinator page with a compact expandable fleet list rather than a bare redirect |

## Scope

The implemented scope covers a deterministic fleet projection and a durable
Coordinator surface that centers the standard conversation experience while
keeping fleet triage visible. The Coordinator remains read-only with respect to
Phoenix-wide data access and does not expose filesystem mutation, workspace
management, or task-approval actions.

## Out of Scope

- Ambient Phoenix-wide history tools for ordinary coding agents.
- Semantic/vector retrieval beyond the existing conversation-retrieval
  substrate.
- Managed workspaces, files, or task lifecycle actions from the Coordinator.
- Multiple user-created global analysis sessions.
- A separate bespoke transcript/composer runtime for the Coordinator.
