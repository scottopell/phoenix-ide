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


## Implementation milestone — source and focused regressions

- Global now consumes the same history page adapter as ordinary conversations through a dedicated Coordinator-only snapshot API. The API/DB read has no ordinary Close or lifecycle authority; ordinary mutation gates are unchanged.
- Continuation records retain their real message IDs, transcript coordinates, timestamps, and chronology. Content-free aggregate boundary metadata supplies unloaded native navigation stops. The existing `ConversationNav`/`PillStrip` is the native web boundary surface; there is no separate `BreadcrumbBar` module in this tree.
- The transcript viewport is aggregate-owned, while the latest writable member owns the embedded composer. Out-of-order older-page completion does not replace an already-adopted newer continuation projection. Exact pin/hash navigation and browser back/forward use the common positioning path.
- Completed continuation markers are compact native boundary disclosures with summary and exact provenance on demand. Generic Markdown/disclosure parser and find-owner seams are untouched.
- First executed regression: continuation chapter test failed with `prompt,prompt` instead of `prompt,boundary,prompt`; after implementation, all 16 chapter tests passed.
- Focused UI milestone: 131/131 tests across aggregate/Global adapters, chapter selector, navigation stack, and boundary component; bounded no-emit TypeScript and focused ESLint passed.
- Guarded cold `./dev.py codegen` completed at jobs=1, incremental=0. Its reservation was released only after process-group descendants were absent. Local evidence is protected under `.phoenix/evidence/continuous-history/` (Phoenix/server-local harness pointer, not portable evidence).
- Still in progress: focused backend regression execution, native desktop/mobile rendered acceptance, independent exact-target review, integrated final-head CI, and reconciliation with the actual-main wake removal. No merge, deployment, or production operation has been performed by this owner.

## Review and representation follow-through

- Review fixes keep reserved-but-unaccepted successor openings nonwritable, lazily position transcript-only historical pins, reissue Back/Forward positioning after hashless entries, and fence delayed prefix restoration against newer navigation commands.
- The ordinary web history API explicitly selects `boundary_representation=original_messages`. Default ordinary snapshots retain handoff metadata for existing Swift consumption; no Swift implementation changes. History segments structurally omit parallel handoff summaries (ADR-100).
- Fixtures now mock the real history methods and mount the real Global shared adapter with a Coordinator-only wire shape. Both ordinary and Global three-member scenarios start at newest-only history and acquire two exact cursor pages only on boundary selection.
- Focused light validation: 144 tests passed across seven history/fixture files; added lazy fixture witnesses subsequently passed 10/10, and finalization adapter regressions passed 11/11. These are intermediate-source results, not final-head qualification.
- Chromium captures passed both ordinary/Global scenarios at desktop/mobile in light/dark, including unloaded and loaded boundary selection, compact/expanded original summary and exact provenance, actual browser Back/Forward through the fixture HashRouter transport, and sending to the newest member. This does not claim production-path cold reload, physical-device acceptance, live continuation/anchor matrix, or final-head acceptance. Dependencies were reused without mutating the shared node_modules symlink; pinned isolated dependency qualification remains separate.
- Read-only backend challenge found no actionable defect in snapshot SHA256 `71243a5650d925d525aa6d95d440e6f0906fd1b271d69971d013263ac92324b3`; no tests were executed by that reviewer.
- Genuine representation codegen exposed two unresolved Rust type paths; they were fixed after exit1 and confirmed descendant absence/reservation release. The owned-target guarded retry completed `./dev.py codegen` with exit0 and verified descendant absence/reservation release; regenerated wire types pass no-emit TypeScript. Generated files are never hand-edited.
- Remaining: executed backend and broad checks, cumulative exact-head review/CI, full live journey/anchor matrix, and deliberate main integration including wake removal and worker-budget controls. Global alone owns merge/deploy.

- Further independent UI review validated a same-route reader-intent race: the earlier-page response captured an obsolete anchor even after the reader selected a new loaded chapter or scrolled. The shared aggregate adapter now consumes updated restore bases; loaded chapter navigation retires the old basis before jumping. Four ordinary/Global regressions reject the stale-basis mutation, and the page/stack focused tests pass 95/95 after restoration.
- Native source-call qualification additionally corrected fixture requested-member identity and exercised ordinary exact-member `/c` routing and Global `/global` routing, with click and cold fixture reload. Chromium and WebKit each passed 16 desktop/mobile light/dark combinations across lazy history and source-call scenarios. This remains fixture-transport evidence, not production-path or physical-device acceptance.
- Full UI suite intermediate source passed 2,907 tests with two deliberate skips across 154 files. Earlier failures were classified: alias navigation was an introduced fixture default-API omission (fixed); production CSS was a Node20 launcher failure (passed alone under Node26). The source changed afterward for the independently found reader-intent fix; final-head full qualification must rerun.
- Spec-shape failure on the old base is an integration dependency, not permission to fill ADR gaps: ADR098 is in actual main's wake retirement, ADR099 remains disclosure-owned and unlanded, ADR100 is retained provisionally. Do not fabricate placeholders or import an unqualified disclosure candidate solely to satisfy sequencing.
