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
available. Continuation topology is reconstructed before visibility filtering,
so archived historical members do not change a chain's durable root. Inclusion
requires positive evidence: open task status or an active/attention state for
Work mode, and active/attention state or activity within 14 days for Direct,
Explore, and Branch modes.

Recall sessions are stored as dedicated global-recall entities rather than as
ordinary conversations. The answering loop uses the existing
`MessageRetriever` with `RetrievalScope::Global` for ranked search and a
separate bounded read path for already-identified conversations. Tool scope is
fixed by the host; the model supplies queries and references, not its own
capabilities. The UI exposes the `/global` surface with open-work browsing,
copyable references, paged open-work/session/transcript lists, saved session
selection, and a recall composer.

## Status Summary

| Requirement | Status | Notes |
|---|---|---|
| **REQ-GR-001:** View Active Work Without Model Inference | ✅ Complete | `/api/global/open-work` builds a deterministic projection; `/global` renders it without model calls |
| **REQ-GR-002:** Collapse Continuation Chains Into One Work Item | ✅ Complete | Projection reconstructs durable `continued_in_conv_id` topology before visibility filtering; covered by archived-root chain identity tests |
| **REQ-GR-003:** Explain Why Work Appears | ✅ Complete | Positive-evidence inclusion and closed-state suppression are table-tested; items expose recency, mode, state, task, and chain signals |
| **REQ-GR-004:** Surface Work Identity Metadata | ✅ Complete | Items expose mode/task/branch/worktree/current/root/update metadata when present |
| **REQ-GR-005:** Provide Stable References and App-Local Links | ✅ Complete | Open-work rows expose durable `@work:` handles; resolution reconstructs historical source/status after closure, and source reads expose app-relative conversation/message links |
| **REQ-GR-006:** Create Saved Read-Only Recall Sessions | ✅ Complete | Dedicated `global_recall_sessions` and `global_recall_messages` tables; ordered independent turns are tested and session/transcript APIs are paged |
| **REQ-GR-007:** Restrict Global Tools to Global Recall Sessions | ✅ Complete | Global search/read/open-work/reference tools are constructed only inside the Global Recall answering loop |
| **REQ-GR-008:** Answer With Source Citations | ✅ Complete | Search/read tools return conversation/message ids and app-local links; hidden-message search exclusion is regression-tested; system prompt instructs citation use |
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
