# Preserve continuation and compaction affordances after ProductConversation canonicalization

## Priority and observed production journey

P0. At a 390x844 production viewport, opening standalone exhausted conversation `saturday-morning-tiger-mountain` presents the context-full handoff with Continue, Edit/review, and Copy actions. Opening exhausted alias `retire-coordinator-final-message-20` instead canonicalizes `/c/:slug` to ProductConversation `6ed105cb-2c17-4405-948e-50b9f30850e8`; the aggregate/latest-segment surface then contains no `.context-exhausted-handoff` and no continuation controls. This is structural absence, not below-fold CSS.

The delivery roadmap (#651) could not be verified during read-only exploration because network access was blocked. Re-check it when write-capable work begins if GitHub is available. Do not deploy and do not touch #745 or #702.

## Normative authority and verified failure model

Read before editing:

- `specs/bedrock/requirements.md`, especially REQ-BED-019 through REQ-BED-021 and REQ-BED-030/030A/031
- `specs/bedrock/bedrock.allium` and `specs/bedrock/executive.md`
- `specs/conversation-ui/requirements.md` and `specs/conversation-ui/executive.md`, especially REQ-CONV-002, REQ-CONV-003, REQ-CONV-007, and REQ-CONV-010
- ADR-026, ADR-031, and ADR-046

The authority split is:

- ProductConversation owns aggregate identity, lifecycle, canonical navigation, and aggregate presentation.
- `continued_in_conv_id` topology derives the latest parent transcript row.
- The latest transcript row owns execution state, messages, continuation state, and continuation mutations.

`ProductConversationAliasRedirect` resolves `/c/:slug` to `snapshot.canonical_route`. `ProductConversationPage` knows `latest_transcript_row_id`, embeds that row with `showTranscript={false}`, and receives `EmbeddedConversationProjection.convState`. Historical completed handoffs are separately flattened into typed system boundary messages. `ConversationPage` currently owns the state-driven `ContextExhaustedHandoff` and awaiting-continuation presentation. Existing ProductConversation tests mock `EmbeddedConversationPage`, so they can pass without proving the real continuation presentation is mounted.

Three fresh read-only reviews covered (1) journey/state authority, (2) canonical route and aggregate composition, and (3) mobile accessibility/layout/YAGNI. Their leading claims were challenged against code. A CSS/scroll-owner explanation is falsified for the reported structural absence: CSS cannot expose a component that is not mounted. No timer or auto-scroll workaround is appropriate. The viable minimal seam is authoritative latest-row state projected into one shared typed continuation presentation; historical handoff metadata must not become a second live-state authority.

## Required implementation

1. Reproduce locally through the canonical alias/aggregate journey (or an equivalent deterministic seeded/fixture journey) and pin the precise mount failure before changing code.
2. Extract or reuse a typed shared presentation boundary for the latest row's non-composer continuation states. Keep continuation request/navigation/copy handlers and authoritative state derivation in the existing conversation owner; do not reimplement `ConversationPage` state logic in `ProductConversationPage`.
3. Make ProductConversation aggregate presentation explicitly render the relevant latest-row continuation state from `EmbeddedConversationProjection` (or make the embedded latest-row presenter own it in a way the aggregate cannot suppress). Preserve a single owner and a single mounted instance.
4. Preserve these state-specific outcomes:
   - **Latest row is `context_exhausted`, no successor:** the generated immutable handoff is prominent and Continue unchanged, Edit/review, and Copy handoff are visible and reachable. Continuation uses the existing exact-once handler and local edit behavior.
   - **Latest row is `awaiting_continuation`:** compacting/progress presentation is visibly mounted; an ordinary composer must not imply that new messages are accepted.
   - **Exhausted predecessor has a successor:** its historical segment boundary/status remains understandable, but the historical handoff must not replace, overlay, or duplicate the successor/latest composer. Existing-successor navigation remains single-successor behavior where applicable.
5. Keep Open/History lifecycle gating and mutation capability explicit and typed. Do not infer live capability from slug equality, flattened system messages, CSS classes, or aggregate message metadata.
6. Keep mobile controls semantic, keyboard accessible, and directly tappable with at least 44px touch targets where REQ-CONV-010 applies. Ensure one deliberate scroll owner makes the full review/copy surface reachable without hiding the primary continuation action.
7. Update a timeless normative requirement only if implementation reveals the aggregate-route obligation is genuinely ambiguous; otherwise rely on REQ-BED-021/030A and add traceable regression coverage without status prose in timeless specs.

Likely starting symbols (not a mandated file list):

- `ProductConversationAliasRedirect` in `ui/src/App.tsx`
- `ProductConversationPageInner`, `makeAggregateMessages`, and `aggregateConversationState` in `ui/src/pages/ProductConversationPage.tsx`
- `EmbeddedConversationPage`, `EmbeddedConversationProjection`, `ConversationPageContent`, and the current continuation handlers/presentation in `ui/src/pages/ConversationPage.tsx`
- `ContextExhaustedHandoff` and its colocated CSS
- `ProductConversationPage.test.tsx`, `App.alias.test.tsx`, `ConversationPage.archived.test.tsx`, and continuation component tests

## Regression evidence

Add focused regressions that fail on the pre-fix implementation:

1. **Canonical aggregate regression:** entering through `/c/:slug` resolves to `/product-conversations/:id`, retains authoritative latest-row state, and mounts exactly one proper continuation/compaction presentation.
2. **Three-state component/page matrix:** cover latest exhausted/no successor, awaiting continuation, and exhausted predecessor/existing successor with a writable latest successor. Assert absence of duplicate owners and that historical state does not displace the latest composer.
3. **Real mobile journey at 390x844:** use the project browser/fixture approach with production-representative aggregate composition. Assert the relevant controls/progress are not merely in the DOM: inspect geometry and computed visibility, scroll the owning region as needed, and prove Continue/review/copy are within the viewport and actionable/reachable. Do not satisfy this with a synthetic DOM-presence assertion.
4. **Desktop non-regression:** prove the aggregate transcript, boundary marker, and latest composer/continuation presentation retain their intended desktop composition.

Prefer the smallest deterministic fixture or browser journey that mounts the real ProductConversation composition; do not extend an unrelated fixture solely because it is mobile. Keep focused Vitest coverage alongside the browser assertion.

## Validation and delivery

- Run focused Vitest suites for alias canonicalization, ProductConversation composition, ConversationPage/shared continuation presentation, and `ContextExhaustedHandoff`.
- Run the 390x844 browser journey and desktop non-regression journey; preserve screenshots or machine-readable geometry assertions as appropriate.
- Run `./dev.py check`.
- Commission a fresh adversarial review of the exact dirty diff after tests. Address or explicitly falsify every finding, with special attention to duplicate state authority, historical/live confusion, missing mobile reachability, and unnecessary abstractions.
- Mark this task `done`, commit and push the completed unit, and open a PR.
- Verify the PR at the exact pushed HEAD: inspect all required CI checks and Codex review output, address valid findings, rerun affected checks, and report final exact-HEAD status. Do not deploy.

## Acceptance criteria

- The production-shaped exhausted alias journey cannot lose continuation controls merely because canonical navigation selects ProductConversation presentation.
- At 390x844, the no-successor handoff's Continue, review/edit, and Copy actions are visibly reachable and actionable.
- `awaiting_continuation` visibly communicates compacting progress on the aggregate route.
- A predecessor with an existing successor remains an understandable historical boundary and never replaces or duplicates the latest successor composer.
- Standalone conversation behavior and desktop aggregate behavior do not regress.
- There is no duplicated continuation state machine, live-state inference from historical messages, timer, or auto-scroll workaround.

## Explicit non-goals

- No backend continuation lifecycle redesign or duplicate aggregate-native execution state.
- No broad ProductConversation visual redesign.
- No production deployment.
- No changes to #745 or #702.
