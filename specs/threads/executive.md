# Phoenix Threads — Executive Summary

## Requirements Summary

Phoenix Threads makes long chains of continuation conversations queryable as
a unit. A user who has run a stream of related work — e.g., conv #41
continued into #42 continued into #44 — can navigate to that chain as a
*thread* and ask recall questions ("what optimizations did we apply?")
without continuing any existing conversation and without re-explaining
scope. Q&A history persists per thread so users can return and review.
From any answer, the user can kickstart a new conversation that inherits
the answer as seed content but diverges from the thread (it does not
extend the continuation chain). Kickstart is for *new direction in the
same topic*, distinct from the existing "continue where we left off"
action. Threads emerge automatically from continuation lineage — no manual
grouping action — and standalone conversations remain ungrouped.

## Technical Summary

Threads are a derived primitive over the existing
`conversations.continued_in_conv_id` graph; no `threads` table is
introduced. Membership is computed by walking the chain. Q&A persists in
a new `thread_qa` table (one row per question/answer pair, indexed by
`root_conv_id`). Each Q&A invocation receives bundled thread context — for
non-leaf members, the existing `MessageType::Continuation` message at the
start of the next conversation in the chain; for the leaf, the transcript
or an on-demand summary — plus the current question, but never prior Q&A
history, which bounds cost and prevents drift. Token streaming reuses the
existing SSE infrastructure on a thread-scoped channel. Kickstart reuses
the seeded-conversations mechanism (`seed_parent_id`, `seed_label`,
review-first draft prompt) with the Q&A answer as draft source. Q&A model
is Claude Sonnet 4.6.

## Status Summary

| Requirement | Status | Notes |
|---|---|---|
| **REQ-THR-001:** Recall Past Work Without Re-Explaining Context | ❌ Not Started | Headline benefit; satisfied jointly by REQ-THR-002 through REQ-THR-006 |
| **REQ-THR-002:** Continuation Chains Surface as Threads | ❌ Not Started | Sidebar grouping; derived from `continued_in_conv_id` |
| **REQ-THR-003:** Thread Page as a Navigable Place | ❌ Not Started | New route `/threads/:rootConvId` |
| **REQ-THR-004:** Ask the Thread, Get a Streamed Answer | ❌ Not Started | Sonnet 4.6; reuses SSE token-stream infrastructure |
| **REQ-THR-005:** Q&A History Persists Per Thread | ❌ Not Started | New `thread_qa` table |
| **REQ-THR-006:** Consistent Quality As Q&A Accumulates | ❌ Not Started | Stateless per-question invocation; no prior Q&A in context |
| **REQ-THR-007:** Kickstart a New Conversation From an Answer | ❌ Not Started | Reuses REQ-SEED-001 |
| **REQ-THR-008:** Kickstart Diverges, Does Not Continue | ❌ Not Started | Reuses REQ-SEED-003 / REQ-SEED-004 |

## v1 (MVP) Scope

All eight requirements ship together. The user story is internally
consistent only when navigation, Q&A, persistence, and kickstart are all
present — partial delivery would expose a half-feature (e.g., navigation
without Q&A, or Q&A without a way to act on the answer). Sub-milestones
inside v1 are tracked as tasks under `tasks/`.

## Out of Scope (Tracked for Future)

- Post-hoc thread membership editing
- Thread renaming (v1 uses root conversation title)
- Q&A editing and deletion
- Follow-up Q&A that references prior Q&A as model context
- Cross-thread linking beyond the single kickstart breadcrumb
- Project-level summary or steering doc (separate, explicitly-deferred concept)
- Allium behavioral spec for thread Q&A lifecycle (pre-token / streaming / persisted, mid-stream failure, concurrent Q&A across tabs). The lifecycle has enough states to warrant a `.allium` distillation — recommended as a follow-up after v1 ships and the actual transitions are observable
