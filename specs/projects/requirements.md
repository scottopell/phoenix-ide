# Projects — Retired Traceability Index

`Project` is retired as a normative domain, product, UI, and API authority. The standing repository-backed authority now lives in `specs/git-repository/`; conversation creation, lifecycle, UI, and tool-capability rules live in their owning specs.

This file is a traceability index only. It deliberately does **not** duplicate normative clauses.

## Moved REQ-PROJ IDs

| REQ ID | New owner |
|---|---|
| REQ-PROJ-000 | `specs/conversation-creation/requirements.md` |
| REQ-PROJ-001 | `specs/conversation-creation/requirements.md` |
| REQ-PROJ-001A | `specs/conversation-ui/requirements.md` |
| REQ-PROJ-002 | `specs/conversation-creation/requirements.md` |
| REQ-PROJ-003 | `specs/bedrock/requirements.md` |
| REQ-PROJ-004 | `specs/bedrock/requirements.md` |
| REQ-PROJ-005 | `specs/conversation-creation/requirements.md` |
| REQ-PROJ-005A | `specs/conversation-creation/requirements.md` |
| REQ-PROJ-006 | `specs/bedrock/requirements.md` |
| REQ-PROJ-007 | `specs/bedrock/requirements.md` |
| REQ-PROJ-008 | `specs/subagents/requirements.md` |
| REQ-PROJ-012 | `specs/bedrock/requirements.md` |
| REQ-PROJ-013 | `specs/bash/requirements.md` |
| REQ-PROJ-015 | `specs/git-repository/requirements.md` |
| REQ-PROJ-017 | `specs/conversation-creation/requirements.md` |
| REQ-PROJ-018 | `specs/bedrock/requirements.md` |
| REQ-PROJ-019 | `specs/conversation-ui/requirements.md` |
| REQ-PROJ-020 | `specs/git-repository/requirements.md` |
| REQ-PROJ-021 | `specs/git-repository/requirements.md` |
| REQ-PROJ-022 | `specs/conversation-creation/requirements.md` |
| REQ-PROJ-024 | `specs/git-repository/requirements.md` |
| REQ-PROJ-025 | `specs/git-repository/requirements.md` |
| REQ-PROJ-028 | `specs/conversation-creation/requirements.md` |
| REQ-PROJ-028a | `specs/work-lifecycle/requirements.md` |
| REQ-PROJ-029 | `specs/conversation-creation/requirements.md` |
| REQ-PROJ-033 | `specs/bedrock/requirements.md` |
| REQ-PROJ-036 | `specs/bedrock/requirements.md` |
| REQ-PROJ-038 | `specs/file-explorer/requirements.md` |
| REQ-PROJ-WS-001 | `specs/work-lifecycle/requirements.md` |

## Explicit deprecations retained for traceability only

### REQ-PROJ-009: DEPRECATED

**DEPRECATED.** Historical lifecycle authority moved long ago; the old squash-merge contract remains retired.

### REQ-PROJ-010: DEPRECATED

**DEPRECATED.** The Project-owned abandon command is retired. ProductConversation Close and History now own the surviving user need in `specs/work-lifecycle/` and `specs/bedrock/`.

### REQ-PROJ-011: DEPRECATED

**DEPRECATED.** Project-owned PR health is retired. WorkScope-associated PR observation and advisory guidance now own the surviving user need in `specs/pr-association/` and `specs/work-actions-bar/`.

### REQ-PROJ-014: DEPRECATED

**DEPRECATED.** Project switchers, grouping, tabs, and active counts are retired product behavior. Conversation capability/lifecycle visibility and repository filtering remain owned by `specs/conversation-ui/`; task counts remain owned by task surfaces. No repository-level grouping/count replacement is defined.

### REQ-PROJ-016: DEPRECATED

**DEPRECATED.** Historical standalone naming.

### REQ-PROJ-023: DEPRECATED

**DEPRECATED.** Reserved compatibility slot.

### REQ-PROJ-026: DEPRECATED

**DEPRECATED.** Branch-mode lifecycle verbs are retired. ProductConversation Close/History and WorkScope retirement own the surviving lifecycle need in `specs/bedrock/` and `specs/work-lifecycle/`; branches remain observed repository state.

### REQ-PROJ-027: DEPRECATED

**DEPRECATED.** Managed-mode completion is retired. ProductConversation Close/History and WorkScope retirement own the surviving lifecycle need in `specs/bedrock/` and `specs/work-lifecycle/`.

### REQ-PROJ-030: DEPRECATED

**DEPRECATED.** Historical Project-scoped PR freshness wording. Current normative authority lives in `specs/pr-association/`.

### REQ-PROJ-031: DEPRECATED

**DEPRECATED.** Historical Project-scoped PR context wording. Current normative authority lives in `specs/pr-association/`.

### REQ-PROJ-032: DEPRECATED

**DEPRECATED.** Historical Project-scoped PR refresh wording. Current normative authority lives in `specs/pr-association/`.

### REQ-PROJ-034: DEPRECATED

**DEPRECATED.** Reserved compatibility slot.

### REQ-PROJ-035: DEPRECATED

**DEPRECATED.** Reserved compatibility slot.

### REQ-PROJ-037: DEPRECATED

**DEPRECATED.** Reserved compatibility slot.

## Retirement note

Historical references to `Project` in older ADRs and executives remain valid as point-in-time records. New normative repository-backed behavior must cite the owning destination spec above, not this file.
