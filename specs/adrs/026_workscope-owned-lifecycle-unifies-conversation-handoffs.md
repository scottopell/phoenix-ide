# ADR-026: WorkScope-owned lifecycle unifies conversation handoffs and worktree ownership

- **Status:** Accepted
- **Date:** 2026-07-26
- **Affects:** REQ-BED-019, REQ-BED-028, REQ-BED-029, REQ-BED-030, REQ-PROJ-004, REQ-PROJ-015, REQ-PROJ-WS-001, REQ-WL-001, REQ-WL-002, REQ-PRA-000, REQ-CHN-008, REQ-GR-001

## Context

Phoenix now has several ways one unit of work spans multiple conversations: context continuation, Explore-to-Work fresh handoff, and chain navigation over that lineage. The product also has work-affine resources and git state that must not be duplicated or ambiguously owned: worktrees, task branches, PR association history, bash/tmux/browser sessions, and cleanup rights.

The fork in the road is where to attach ownership as a work item moves. One option is to keep ownership on whichever conversation first created the worktree and let later conversations point back to it indirectly. Another is to copy worktree identity and related resource ownership onto each successor. A third is to make the durable work identity first-class and let conversations either own that identity or become history references after handoff.

The existing specs already trend toward one durable work identity: continuations transfer ownership instead of recreating a checkout, work-scope resources are keyed independently of conversation ids, and PR association is WorkScope-owned. What remains is to make that direction explicit and shared so chain, retrieval, cleanup, SSE, and coordinator specs do not drift into mixed models.

## Options considered

1. **Origin-conversation ownership** — keep the original conversation as the durable owner of the worktree and resources forever, even after continuation or fresh handoff. This preserves one row as the anchor but makes live ownership diverge from the conversation the user is actually working in, complicates terminal-action legality, and forces every consumer to special-case "current conversation" versus "owning conversation."
2. **Per-conversation copied ownership** — let every successor persist its own full worktree/resource ownership facts. This makes local reads simple per row, but creates parallel authoritative representations of the same work identity, invites drift between predecessors and successors, and makes cleanup/co-owner rules depend on reconciliation between copied facts.
3. **WorkScope-owned lifecycle with explicit handoff** — persist one durable work identity (`work_scope_id`) for work-affine resources and treat conversation handoff as an ownership transfer. The current live conversation owns the scope; predecessors become history references through `continued_in_conv_id`; sibling surfaces read one scope and one active owner at a time.

## Decision

Phoenix adopts **WorkScope-owned lifecycle with explicit handoff**.

Work-affine resources are owned by the persisted `work_scope_id`, not by a conversation id or by a filesystem path. A conversation may be the current live owner of that scope, but once it hands work to a continuation or fresh Work successor it becomes a historical node in the chain rather than a competing owner. The handoff is durable and explicit: `continued_in_conv_id` records lineage, while the scope continues through the successor without creating a second worktree or a second authoritative resource owner.

This choice wins because it keeps one semantic unit of work represented once. Cleanup, PR targeting, chain work identity, and coordinator orientation can all ask the same question — which live conversation currently resolves this WorkScope? — instead of reconciling copied git metadata or consulting an origin row that the user is no longer acting in. It also matches the repo-wide correct-by-construction rule: one work item has one durable owner identity, while predecessor conversations remain valuable as transcript history and navigation anchors.

## Consequences

- **Positive:** Continuations and fresh Work handoffs reuse the same checkout and resource identity without ambiguous co-ownership. Cleanup rules can preserve or remove a worktree based on whether another live conversation still resolves the same WorkScope. PR association, chain identity, and coordinator orientation can present one work item consistently across multiple conversations.
- **Negative:** Surfaces that once treated a conversation row as the sole owner of work metadata must now resolve through WorkScope and distinguish live owner from historical predecessor. Specs and implementations must be careful not to derive ownership from `continued_in_conv_id`, working directory, or copied branch fields alone.
- **Neutral:** Chain lineage and WorkScope are related but not identical concepts. In the common case a linear chain maps to one WorkScope, but the product still keeps both abstractions: lineage for navigation/history, WorkScope for resource ownership and work-affine state.

## References

- Related ADRs: ADR-008, ADR-020, ADR-023, ADR-024
- Specs: `specs/bedrock/requirements.md`, `specs/projects/requirements.md`, `specs/work-lifecycle/requirements.md`, `specs/pr-association/requirements.md`, `specs/chains/requirements.md`, `specs/global-recall/requirements.md`
- Symbols: `Conversation.continued_in_conv_id`, `WorkScope`, `TaskResolved`, `SseBroadcaster::send_seq`
