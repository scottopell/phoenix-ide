# Phoenix Global Recall — Executive Summary

## Requirements Summary

Phoenix Global Recall gives users a deliberate global surface for orientation
and cross-conversation analysis. The Global Open Work view answers "what active
work exists?" deterministically by grouping visible work by project and
representing each open work item as either a continuation chain or standalone
conversation. Each item carries metadata and explainable signals so the user can
see why it appears.

Separate saved Global Recall sessions provide read-only synthesis over Phoenix
history. They are distinct from ordinary coding conversations, can coexist, and
can use host-bound read-only tools for global message search, paged source
conversation reads, deterministic open-work reads, and reference resolution.
Answers are expected to cite source conversations or messages using app-local
links or stable handles.

## Technical Summary

The open-work projection derives from persisted conversations, project rows,
continuation links, conversation modes, runtime states, and task metadata when
available. Continuation chains are collapsed under their root and sorted by the
latest member's update time; archived and non-user-initiated conversations are
excluded.

Recall sessions are stored as dedicated global-recall entities rather than as
ordinary conversations. The answering loop uses the existing
`MessageRetriever` with `RetrievalScope::Global` for ranked search and a
separate bounded read path for already-identified conversations. Tool scope is
fixed by the host; the model supplies queries and references, not its own
capabilities. The UI exposes the `/global` surface with open-work browsing,
copyable references, saved session selection, and a recall composer.

## Status Summary

| Requirement | Status | Notes |
|---|---|---|
| **REQ-GR-001:** View Active Work Without Model Inference | ✅ Complete | `/api/global/open-work` builds a deterministic projection; `/global` renders it without model calls |
| **REQ-GR-002:** Collapse Continuation Chains Into One Work Item | ✅ Complete | Projection walks `continued_in_conv_id` chains and emits one item per root/latest lineage |
| **REQ-GR-003:** Explain Why Work Appears | ✅ Complete | Items expose signal strings for recency, mode, active/attention states, task status, and chain membership |
| **REQ-GR-004:** Surface Work Identity Metadata | ✅ Complete | Items expose mode/task/branch/worktree/current/root/update metadata when present |
| **REQ-GR-005:** Provide Stable References and App-Local Links | ✅ Complete | Items expose app-relative `href` plus `@chain:` / `@conv:` references; source message reads include message anchors when slugged |
| **REQ-GR-006:** Create Saved Read-Only Recall Sessions | ✅ Complete | Dedicated `global_recall_sessions` and `global_recall_messages` tables; `/global` supports multiple saved sessions |
| **REQ-GR-007:** Restrict Global Tools to Global Recall Sessions | ✅ Complete | Global search/read/open-work/reference tools are constructed only inside the Global Recall answering loop |
| **REQ-GR-008:** Answer With Source Citations | ✅ Complete | Search/read tools return conversation/message ids and app-local links; system prompt instructs citation use |
| **REQ-GR-009:** Resolve Copied References | ✅ Complete | `/api/global/resolve` supports typed Global Recall handles and app-local conversation/chain paths |

## Scope

The implemented scope covers a deterministic open-work projection and manually
created saved recall sessions. Recall sessions are read-only and have no managed
workspace, filesystem mutation tools, task approval tools, task drafting tools,
automatic continuation, or automatic summarization.

## Out of Scope

- Ambient global-history tools for ordinary coding agents.
- Semantic/vector retrieval beyond the existing conversation-retrieval
  substrate.
- Automatic global-session summarization or continuation management.
- Managed workspaces, files, or task lifecycle actions from Global Recall
  sessions.
- An Allium behavioral model. The present behavior is projection plus simple
  saved-session lifecycle; an Allium spec becomes worthwhile if Global Recall
  gains branching session states, task-approval flows, or partial-failure
  recovery semantics.
