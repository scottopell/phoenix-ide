# Phoenix Coordinator — Executive Summary

## Requirements Summary

Phoenix Coordinator is one durable, chat-first Phoenix-wide conversation for surveying unrelated work, inspecting relevant history, and sending useful text guidance to existing conversations. Phoenix supplies deterministic current-work orientation on every Coordinator turn, while the Coordinator retains bounded global search, conversation reading, open-work querying, and stable reference resolution.

The Coordinator has one mutation capability: a singular text-message action targeting an existing non-Coordinator conversation. That action reuses the normal chat acceptance authority, so each target independently reports delivered, queued as steering, or rejected. Acceptance never implies that the receiving agent understood, acknowledged, or completed the instruction.

The `/global` surface is the standard transcript and composer without a separate open-work view. A compact composer action requests a read-only current-work briefing through the normal message path while preserving the user's draft.

## Technical Summary

The open-work projection derives from persisted conversations, project rows, continuation links, conversation modes, runtime states, and task metadata. Continuation topology is reconstructed before visibility filtering, so archived historical members do not change a chain's durable root. Inclusion requires positive evidence: open task status or an active/attention state for Work mode, and active/attention state or activity within 14 days for Direct, Explore, and Branch modes.

Coordinator LLM requests keep the stable language-specific prompt as their cached prefix and append a bounded, turn-current work capsule. The capsule deterministically prioritizes attention/recovery, active, and recently idle work, provides aggregate counts, and identifies truncation. The complete paginated open-work tool remains available when the capsule is insufficient.

A shared application service owns user-message acceptance and dispatch for both the ordinary HTTP chat adapter and the Coordinator tool. It preserves message and steering idempotency, live runtime authority, stored-state rejection, runtime materialization, acceptability checks, steering depth limits, persistence, broadcast behavior, and PR auto-fix baseline behavior. Typed semantic outcomes prevent normal delivery, steering acceptance, and rejection from being confused.

The Coordinator registry is builtin-only. It contains bounded global read tools and the singular cross-conversation text-message tool, while ordinary conversations do not receive those capabilities. The Coordinator continues to exclude shell, filesystem, browser, MCP, repository, task, project, workspace, conversation creation, approval, and lifecycle mutation tools.

## Status Summary

| Requirement | Status | Notes |
|---|---|---|
| **REQ-GR-001:** Provide Deterministic Current Work | ✅ Complete | Each Coordinator turn receives the deterministic projection without a separate browser work view |
| **REQ-GR-002:** Collapse Continuation Chains Into One Work Item | ✅ Complete | Projection reconstructs durable `continued_in_conv_id` topology before visibility filtering |
| **REQ-GR-003:** Explain Current Inclusion and Attention | ✅ Complete | Positive-evidence inclusion and attention classification derive from current states and signals |
| **REQ-GR-004:** Provide Bounded Work Selection Data | ✅ Complete | The Coordinator-only open-work tool filters before bounded pagination and exposes selection data |
| **REQ-GR-005:** Provide Stable References and App-Local Links | ✅ Complete | Open-work tool results expose durable `@work:` handles and app-local destinations; resolution reconstructs historical source/status |
| **REQ-GR-006:** Provide One Durable Coordinator Identity | ✅ Complete | `/api/global/coordinator` resolves the singleton conversation through the standard runtime and UI |
| **REQ-GR-007:** Bound Phoenix-Wide Coordinator Capabilities | ✅ Complete | The builtin-only Coordinator registry adds one text-message action while ordinary registries remain unchanged |
| **REQ-GR-008:** Answer With Source Citations | ✅ Complete | Global reads expose source metadata and the Coordinator prompt requires stable source citations |
| **REQ-GR-009:** Resolve Durable Targets Without Guessing | ✅ Complete | Typed message resolution supports work/conversation handles, app-local conversation links, and ids with chain checks |
| **REQ-GR-010:** Keep the Coordinator Surface Chat-Only | ✅ Complete | `/global` mounts only the shared conversation runtime and an inline read-only briefing action |
| **REQ-GR-011:** Inject Bounded Current-Work Context | ✅ Complete | Coordinator requests append deterministic turn-current context after the cached stable prompt |
| **REQ-GR-012:** Commit and Report One Message Outcome | ✅ Complete | HTTP chat and the Coordinator action share typed acceptance and report delivery, steering, or stable rejection |

## Verification Summary

Existing coverage verifies durable Coordinator routing, normal transcript mounting, continuation-chain projection, positive-evidence open-work inclusion, stable references, bounded reads, and Coordinator-only global read tools.

Required verification for the actionable console covers immediate delivery, busy steering, persisted and queued idempotent retries, stable rejection outcomes, continuation target resolution, Coordinator-chain rejection, independent multi-target calls, ordinary-conversation permission isolation, deterministic capsule ordering and truncation, filter-before-pagination, chat-only responsive layout, and the inline briefing action's normal composer semantics.

## Scope

The scope is a deterministic current-work projection, one durable chat-only Coordinator conversation, bounded global reads, automatic turn-current orientation, one singular text-message action to existing non-Coordinator conversations, and a compact read-only briefing action.

The Coordinator runs only on user turns. It does not monitor work in the background, create conversations, manage a global objective, retain attention history, or infer recipient understanding from message acceptance.

## Out of Scope

- Ambient Phoenix-wide tools for ordinary coding agents.
- Images, files, skills, user-agent metadata, or lifecycle commands in cross-conversation messages.
- Batch-action transactions or atomic fan-out.
- Conversation creation, task lifecycle, approval, repository, filesystem, project, or workspace mutation from the Coordinator.
- Durable attention/outcome history, observation cursors, exact changes-since-last-turn context, and desktop notifications.
- Semantic/vector retrieval, saved find-work views, LLM-powered utility filtering, or a utility cache.
- Background monitoring or proactive intervention without a user turn.
- Multiple user-created global analysis sessions.
- A separate transcript/composer runtime for the Coordinator.
