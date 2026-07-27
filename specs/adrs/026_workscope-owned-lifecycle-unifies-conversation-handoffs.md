# ADR-026: Product conversation lifecycle is separate from WorkScope resource ownership

- **Status:** Accepted
- **Date:** 2026-07-26
- **Affects:** REQ-BED-019, REQ-BED-028, REQ-BED-029, REQ-BED-030, REQ-PROJ-004, REQ-PROJ-015, REQ-PROJ-WS-001, REQ-WL-001, REQ-WL-002, REQ-PRA-000, REQ-CHN-008, REQ-GR-001

## Context

Phoenix's unified conversation work had reached a point where one draft ADR was trying to explain several distinct facts with one ownership claim: continuations remain one product conversation, transcript rows remain the durable execution segments, `WorkScope` owns work-affine resources, and branches and pull requests remain observed repository state. That draft direction was too coarse. Saying that `WorkScope` owns the lifecycle blurred product lifecycle, transcript topology, and resource ownership into one dimension.

The affected requirements now distinguish those dimensions explicitly:

- the durable **product conversation** is the user-facing thing that is Open or History;
- durable **`Conversation` rows** are transcript and execution segments linked linearly by `continued_in_conv_id`;
- attached **`WorkScope`** values own worktrees and other work-affine resources;
- **Git-backed** versus **chat-only** describes environment intent, not lifecycle ownership;
- branches and pull requests are repository facts Phoenix observes rather than lifecycle artifacts Phoenix owns.

The point-in-time decision needed here is not whether Phoenix should invent another aggregate or cache table. It is which persisted identity owns which kind of truth, so later requirements and Allium rules do not drift back into mixed authority.

## Options considered

1. **Let transcript rows own lifecycle and latest resolution.** Each durable `Conversation` row would carry enough lifecycle meaning to answer whether the user-facing conversation is open, historical, and current. This keeps everything on one row type, but it duplicates product-lifecycle truth across a chain, makes predecessor read-only status easy to confuse with History, and pushes consumers toward extra "latest" caches or lookup tables.
2. **Let `WorkScope` own the product lifecycle.** A single `WorkScope` could appear to unify continuations, cleanup, and runtime resources. This is appealing because the worktree survives continuation, but it collapses resource ownership into product lifecycle, incorrectly implies that attachment equals cleanup authority, and pressures the model toward a global one-to-one conversation/scope assumption that the product does not want to promise.
3. **Let branch, task, or pull-request state define lifecycle ownership.** The worktree's current branch, a task branch, or an associated pull request could act as the durable anchor for "the work." This matches some legacy wording, but it contradicts ADR-008's branch/PR observation model, makes lifecycle depend on mutable repository state Phoenix does not own, and would force Close or approval flows to mutate or bless repository artifacts as lifecycle transitions.
4. **Treat task spawn or follow-up as ordinary continuation.** Approved-task Start in new conversation and follow-up could extend the same product conversation and reuse the same `WorkScope`. This would preserve one lineage, but it would make fresh work indistinguishable from context continuation, would copy lifecycle and retrieval semantics onto separate units of work, and would violate the approved-task placement contract that Start new is separate and fresh.
5. **Add duplicate root/latest authority tables or cached fields.** Phoenix could keep one product-conversation/root table plus extra latest-resolution tables or row fields to avoid traversing `continued_in_conv_id`. This may simplify some reads, but it creates parallel representations of the same semantic fact and makes divergence between topology and cache a first-class risk.
6. **Separate product lifecycle, transcript topology, and resource ownership.** Product conversations own Open/History lifecycle; transcript rows remain the durable segments and latest resolution is derived from `continued_in_conv_id`; `WorkScope` owns resources and may be attached across rows and subordinate executions without becoming the lifecycle owner.

## Decision

Phoenix separates **product lifecycle**, **transcript topology**, and **resource ownership**.

The **ProductConversation** aggregate is the durable user-facing root, identified by its durable root identity rather than by any one root transcript row, and it is the only owner of the Open/History lifecycle. The Close action transitions that product conversation to History **only after** the committed retirement flow has released every owned resource. A typed `needs-repair` outcome remains an Open-state condition that is retryable; it is not an alternative completion condition for entering History. Close does not classify one transcript row, one branch, or one pull request as "the" lifecycle owner.

Durable **`Conversation` rows** remain transcript and execution segments. Context continuation creates a new row in the **same** product conversation and keeps the same attached `WorkScope`; `continued_in_conv_id` is the sole linear topology for predecessor/successor order and latest-row resolution. Phoenix does not add a second latest/root authority or describe context continuation as new product lifecycle creation, and it avoids framing transcript rows as owning or transferring the `WorkScope` attachment.

Attached **`WorkScope`** values own resources: worktrees, bash/process resources, tmux, browser state, PTY/terminal state, and other work-affine resources. Conversations, execution rows, and sub-agents may have a `WorkScope` attached, but those attachments do not themselves own cleanup. Phoenix therefore does not assert a global one-to-one rule between product conversations and `WorkScope`s beyond the specific contracts the requirements already state.

**Git-backed** and **chat-only** remain environment-intent vocabulary. Git-backed conversations normally attach a `WorkScope`; chat-only conversations do not own Git-backed lifecycle. Whether a chat-only conversation may also carry some non-Git `WorkScope` attachment is intentionally left undecided here; this ADR does not need that decision to separate lifecycle from resource ownership.

Approved-task **Start in new conversation** creates a separate ProductConversation with a fresh `WorkScope` and worktree because that path is Git-backed. **Follow-up** also creates a separate ProductConversation with a fresh environment appropriate to its intent plus a typed source relation: Git-backed follow-up gets a fresh `WorkScope` and worktree, while chat-only follow-up does not fabricate a Git `WorkScope`. They are not continuations. **Continue here** is only a checkpoint inside the same product conversation and same attached `WorkScope`.

Source relation remains a distinct fact from continuation topology, lifecycle ownership, and resource attachment.

Close retirement inspects worktree-loss risk only when no attached scope in the product aggregate owns a Git-backed worktree; mixed attachments do not waive inspection. Retirement then proceeds per owned resource, recording exact-attempt evidence as each resource is retired or found absent before the product conversation may enter History. Same-aggregate continuation rows and subordinate execution conversations do not veto retirement of that attached `WorkScope`; preservation applies only when a distinct still-Open ProductConversation shares the same scope, or when identity conflict prevents Phoenix from proving whether such a distinct aggregate exists.

Branches and pull requests are **observed**, never lifecycle-owned. Phoenix may observe branch and PR facts through `WorkScope`-keyed evidence and may guide the user with those facts, but conversation lifecycle does not own, mutate, or derive authority from branch or PR state.

## Consequences

- **Positive:** Each semantic fact now has one owner. Product lifecycle questions resolve through the product conversation, transcript/latest questions resolve through `continued_in_conv_id`, and cleanup/resource questions resolve through `WorkScope` ownership.
- **Positive:** Context continuation, Close, approved-task placement, PR targeting, unified transcript presentation, and Coordinator orientation can compose without inventing hidden row-lifecycle or branch-lifecycle rules.
- **Positive:** The model preserves ADR-008's branch/PR observation boundary: lifecycle can guide from repository facts without mutating or owning them.
- **Negative:** Consumers must distinguish product vocabulary from implementation vocabulary. A read-only predecessor row inside an Open product conversation is not History, and an attached `WorkScope` is not itself the lifecycle root.
- **Negative:** Some reads must derive latest execution from `continued_in_conv_id` instead of relying on duplicated latest/root tables or cached row ownership fields.
- **Negative:** Consumers must resolve attached scope from the ProductConversation while resolving live execution state from only the latest execution row; inventing a row owner for `WorkScope` would collapse those distinct authorities again.
- **Neutral:** This ADR leaves open whether some chat-only conversations may carry non-Git `WorkScope` attachments, because that question does not change the accepted ownership split.

## References

- Related ADRs: ADR-008, ADR-020, ADR-023, ADR-024
- Specs: `specs/bedrock/requirements.md`, `specs/projects/requirements.md`, `specs/work-lifecycle/requirements.md`, `specs/pr-association/requirements.md`, `specs/chains/requirements.md`, `specs/global-recall/requirements.md`
- Key symbols: `Conversation.continued_in_conv_id`, `WorkScope`, `ProductConversation`
