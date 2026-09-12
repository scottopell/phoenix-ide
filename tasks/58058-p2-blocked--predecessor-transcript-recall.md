# Predecessor transcript recall for continuing parent agents

Implementation child of ProductConversation task 92009, coordinated with gate 6 task 92015. Implements REQ-RET-009 and ADR-048. This task owns only same-conversation predecessor discovery and agent recall; gate 6 retains follow-up source/provenance and UI retrieval ownership.

## Delivery dependency

Wait for ProductConversation integration PR #745 and final integration/QA PR #702 to settle on main before changing runtime registration or prompt construction. Then rebase and confirm the shipped aggregate/member and provider-prompt contracts. This follow-up is not a prerequisite for that integration.

## Observed gap

`coordinator_tools::writing_tools` already registers global search/read for write-capable parents. `SearchConversations` accepts a query only and searches globally; `ReadConversation` needs a known target. Restricted planning parents lack these global tools. `chain_qa` has scoped search/read and orientation for its separate read-only Q&A agent. No ordinary-parent predecessor tool exists. Compaction should make original predecessor evidence discoverable without manual user transfer or operator-level SQL.

## Acceptance criteria

- [ ] Add `previous_transcripts` with closed list/search/read inputs and typed success, no-predecessors, unavailable, invalid-target/cursor, and coverage outcomes under REQ-RET-009.
- [ ] Bind ordinary ProductConversation and executing parent transcript identities at the host; validate authoritative membership and strict predecessor topology per call. Never derive scope from the latest aggregate member, WorkScope, or follow-up source.
- [ ] Register for ordinary write-capable and restricted planning parents, including runtime reconstruction and planning-to-write upgrades. Do not register for subagents or Coordinator; preserve global-tool eligibility separately.
- [ ] Supply bounded host-authored immediate-predecessor orientation and tool guidance through the authoritative provider-prompt path. Reconstruct it after restart; do not depend on the summary model to invent references or inject predecessor bodies.
- [ ] Reuse `MessageRetriever` and in-query `RetrievalScope::Conversations` plus bounded full-content reads. Listing/read must work without a search hit or a ready retrieval index. No new index, transcript store, or duplicated topology authority.
- [ ] Return stable typed transcript/message references, ordered pageable listing, and target-bound read cursors; respect host byte/token budgets including oversized individual messages and provider context limits.
- [ ] Validate an actual continuation journey: a fact omitted by the summary can be found in an earlier transcript through both ranked search and direct paged read. Demonstrate the same journey for a restricted planning parent without global tools.
- [ ] Update executive status only after implementation and verification.

## Regression matrix

- Root with no predecessors versus authoritative search no-match versus unavailable binding/index coverage.
- Three parent transcripts A -> B -> C: C can inspect A/B; a capability bound to B never reads B/C, even after C is appended.
- Same WorkScope in another aggregate, source-linked follow-up, sibling work, Coordinator, and subordinate transcript IDs are rejected without content disclosure.
- Missing/changed membership, deleted data, and invalid/replayed cross-target cursors cannot widen access or substitute another source.
- Search finds an in-scope match despite many stronger global matches; incomplete index coverage is explicit and live read remains usable.
- Long listing pagination, large single-message/tool-output pagination, exact source identities, and all tool-call outcomes stay bounded.
- Runtime restart and planner-to-write transition preserve host binding and discoverability without duplicate tool registration.

## Starting points

`crates/phoenix-ide/src/coordinator_tools.rs`, `api/global_read.rs`, `chain_qa.rs`, `runtime/traits.rs`, `system_prompt.rs`; `crates/phoenix-db/src/retrieval.rs` and `prompt_projection.rs`. Reuse behavior without making the legacy chain page or global operator-level service the new capability authority.
