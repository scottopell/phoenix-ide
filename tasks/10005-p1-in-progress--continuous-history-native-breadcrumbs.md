# Continuous conversation history and native breadcrumb boundaries

## User commission and release gate
Explicit v0.13 FINAL blocker, relayed by Global 12dd4025-e960-47eb-aced-1b3965e7c894: user said “Good, queue this up as 0.13 blocker, get to work on it.” Release acceptance ledger: GitHub issue #780; portfolio #806. This is implementation now, not a planning backlog. Global retains merge/deploy/release admission; do not bundle unqualified work into P1 recovery.

## Goal and acceptance
- Seamless lazy scrolling through all predecessor transcripts in Global AND regular conversations. Preserve visible reader anchor, chronological exact message/transcript IDs, no gaps/duplicates and no stop at opening summary.
- Compact truthful continuation boundaries with summary/provenance available on demand.
- Breadcrumbs natively represent boundaries, follow currently read history, and navigate unloaded older messages/boundaries with automatic loading using the SAME interpretation as scrolling.
- Preserve exact links, query/hash, back/forward navigation, newest-writable composer, and distinct Global lifecycle.
- Qualify multi-continuation desktop/mobile journeys: long history, live append/continuation while reading older content, load failure/retry, old deep links and back/forward, no duplicate/missing records.
- Focused regression tests, rendered journey receipts, exact-head applicable CI and independent cumulative review before Global merge.

## Grounding and implementation boundary
Base be1fbc52c394e299e48268c4a5079dc22851f5ac. CoordinatorPage mounts a single-member ConversationPage; loadOlderMessagesForIntent can mark that member complete. Ordinary ProductConversationPage already uses aggregate cursor/mergeOlderSegments; backend product_conversations has cross-segment no-gap tests. Reuse that substantial existing aggregate machinery. task92013/#725 do not prove this universal actual journey.

Dedicated owner owns CoordinatorPage/ProductConversationPage/ConversationPage history adapters, aggregate loading/navigation/anchor hooks, BreadcrumbBar continuation interpretation and adjacent tests/styles. Backend aggregate API changes only if a demonstrated gap requires them; preserve wire/codegen contracts. Necessary focused specs/design/tests/review are admitted implementation, not routine reapproval.

Disclosure owner58cff20d owns #889 parser/markdown/disclosure rendering. Do not edit its dirty worktree or change parser/MarkdownRenderer/disclosure implementation. Agree exact shared-file hunks before overlapping writes; use adapters/props where appropriate. No broad framework, prompt-context redesign, historical data rewrite, runtime lifecycle redesign or native Swift scope.

## Execution
Isolated branch feat/continuous-conversation-history nested under coordinator evidence; preserve parent dirty tree. Implement through one dedicated owner and focused PR. No production changes. Builds use current host self-service reservations and capacity guards; source/light tests may progress independently. Do not mark done until implemented and qualified; release acceptance remains separately tracked in #780.
