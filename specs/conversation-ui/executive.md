# Conversation UI - Executive Summary

## Scope and Boundary

This spec governs the **conversation experience** — list, chat view, composition, agent-activity display, and the responsive layout that holds them. It does NOT govern the broader UI surface. Phoenix's UI is split across multiple feature specs; readers should not expect this document to describe everything they see in the app.

**In scope here:**
- Conversation list and selection
- Chat view: message history, streaming, conversation navigation, state bar
- Conversation view density (full / compact)
- Conversation navigation strip (whole-conversation chapters)
- Message composition (text, drafts, send/cancel)
- Connection + reconnection visibility
- Sidebar (desktop) / bottom sheet (mobile) for new-conversation entry

**Owned by other specs (cross-references, not duplication):**
- `specs/file-explorer/` — left/right file-tree column on wide desktop
- `specs/command-palette/` — Cmd-K command palette
- `specs/viewer_slot/` — when prose / diff / browser viewer replaces or sits beside the chat column
- `specs/voice-input/` — voice recording + transcription
- `specs/ask-user-question/` — `QuestionPanel`, `TaskApprovalReader` rendering
- `specs/browser-tool/` — `BrowserViewPanel` rendering
- `specs/notifications/` — toast + browser notification policy
- `specs/inline-references/` — `@file`, `/skill` autocomplete inside InputArea
- `specs/keyboard-interaction/` — global keyboard shortcuts

Surfaces that do not yet have their own spec (TerminalPanel, TasksPanel, SkillsPanel, ConversationSettings, RenameDialog, MessageContextMenu, ImageAttachments) are tracked as a follow-up; this document does NOT claim coverage of them just because they live in `ui/src/components/`.

## Project-Retirement Current Reality

The shipped new-conversation and sidebar surfaces still consume legacy Project suggestions/grouping and mode metadata. The normative target keeps recent repository-location suggestions only as a projection of surviving conversation/WorkScope activity (REQ-PROJ-001A), filters conversations without a Project product grouping (REQ-PROJ-019), and never exposes hidden `GitRepository` as a user-owned collection.

## Requirements Summary

Within the scope above: a responsive interface for conversations with the AI agent across mobile and desktop. Users can view and manage conversations, compose messages with draft persistence, and monitor agent activity in real-time. The interface handles unreliable network connectivity gracefully with optimistic UI, automatic reconnection, and offline message queueing. Desktop users get a persistent sidebar layout with conversation list alongside the active chat. New conversation creation goes through a single dedicated route (`/new`) regardless of entry point — sidebar "+ New" navigates there, the responsive layout inside the page adapts to viewport (full-page form on desktop, bottom-sheet styling on mobile). The form supports "Send in Background" for spawning work without navigating away from a previous conversation.

## Technical Summary

React 18 SPA with React Router, Vite build tooling, and CSS variables for theming. Conversation state managed via a single `ConversationAtom` + `useReducer` in a router-level React context — all SSE events flow through one pure reducer, eliminating split-brain between independent `useState` atoms. `ConversationState` is a TypeScript discriminated union with `satisfies never` on every switch — new backend state variants are compile errors. The atom also owns the server-authoritative steering queue: init snapshots restore it after reconnect or restart, live enqueue/cancel events mutate it, and browser-local optimistic entries are deduplicated by message ID. `agentWorking` is a derived selector, not maintained state, and the StateBar is the sole owner of the live "is the agent working right now" answer. `lastSequenceId` lives in the atom and survives navigation, replacing unbounded `seenIdsRef` with O(1) idempotency. Token streaming accumulates in `streamingBuffer` on the atom; the `sse_message` action clears it atomically in one render (REQ-CONV-019 no-flicker). `appMachine.ts` (previously dead code) is wired as the live implementation via `useAppMachine.ts`. A shared `PillStrip` primitive backs the conversation navigation strip and compact-mode tool strip. Conversation view density (`full`/`compact`) is a presentational preference layered over the canonical `buildRenderUnits` output; assistant prose remains fully visible in both densities while compact mode reduces tool chrome. Compact bash cards preserve command or handle identity, lifecycle status, duration, and a bounded structured output tail instead of reducing bash to a generic done badge, and density no longer implies that only finalized turns can render compact tool cards. `buildConversationChapters` uses the same significance threshold to derive the navigation strip's chapters, derives chapter labels from human-meaningful conversation text rather than opaque display IDs, and jumps the virtualized list by render-unit index (`scrollToUnitIndex`) rather than by DOM query.

## Markdown Disclosure Qualification

**IMPLEMENTED; local qualification complete, hosted review/landing pending.** REQ-CONV-025 and [ADR-099](../adrs/099_bounded-markdown-disclosures.md) define the bounded replacement inside the native Markdown stack. The source candidate at `94a64665d27badcdb1aa78973c74753d7734c80b` passed independent exact-head static review, strict source TypeScript, ESLint/Stylelint, and the full UI suite: **156 files, 3,076 tests passed, 2 skipped**. This is branch qualification, not a claim that disclosures are deployed.

The v6 prototype reported **197 passing gates** (129 regression + 68 additional checks), including the original opaque-script/context counterexample, native basic/GFM semantics, forward/global definitions, literal fallback, original source offsets, and raw-HTML-disabled HAST conversion. This is bounded prototype evidence, **not production tests**. Local evidence is retained under `.phoenix/disclosure-spike/REPORT-v6.md`, `SHIPPING-SEAMS.md`, and the v6 result manifests; these are harness-local pointers, not shipped artifacts or a whole-parser complexity proof. The prototype's 150,000-code-unit cap does not establish a product limit.

Actual-source tests cover the original opaque-context failure, cyclic/late references, literal fallback, native code/container ownership, source offsets, boolean-open safety, rendering/streaming/find parity, global footnotes, and completed code/Mermaid memoization. Independent review found and corrected candidate-index suppression by code-span contents and malformed-parent nested promotion; their red and green logs remain retained. Streaming now follows canonical native fence semantics rather than the legacy nested-markdown-fence heuristic; the old utility remains unchanged for other callers.

Static default/development native seam checks and the browser production bundle passed. A real-App Chromium journey with synthetic APIs verified initially collapsed state, Enter to open, Space to close, find opening/highlighting the hidden body, and same-context body-link navigation with no page errors. This is not native iOS or physical-device acceptance. Hosted clean frozen installation/CI and terminal review remain landing gates. Conversation find uses the canonical interpretation; file/plan find retains standard Markdown scope. Raw HTML execution/reparsing remains disabled, and only structural details/summary properties plus boolean `open` reach the renderer.

Harness-local receipts are retained under `.phoenix/disclosure-spike/frozen-*-receipt.json`, `static-seams.log`, and `.phoenix/disclosure-browser/acceptance.json`. An earlier full-suite attempt used a mixed source snapshot during lint extraction and failed its bundle check; it is not counted as qualification. The frozen-head rerun passed. No compiler cache cleanup, production activation, or native campaign is part of these receipts.

An inherited downstream limitation remains unresolved: both the native control and v6 prototype failed native HAST conversion at **4,096 nested blockquotes** with `RangeError: Maximum call stack size exceeded` (`v6-hast-depth.jsonl`). Successful native compilation and iterative application walks do not resolve or qualify that renderer-depth limitation. Extension traversal/submission counters do not count every native speculative read and must not be promoted to a native linear-time or arbitrary-depth safety claim.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-CONV-001:** Conversation List | ✅ Complete | List with slug, cwd, timestamps |
| **REQ-CONV-002:** Chat View | ✅ Complete | Messages, markdown, tool grouping, invisible reader-proximity history acquisition with localized retry; raw and Markdown-encoded local file paths share one classifier and open through the conversation file viewer; browser screenshot attachment URIs render inline through a filename- and canonicalization-confined preview path (`linkifyText`, `ConversationMarkdownAnchor`, `resolveConversationMarkdownImageSrc`, `MessageComponents.test.tsx`) |
| **REQ-CONV-003:** Message Composition | ✅ Complete | One-line mobile composer that grows to its bounded maximum; draft persistence; Enter/Shift+Enter and IME-safe submission; compact non-overlapping icon controls; concurrent Stop and steering Queue actions while busy; desktop conversation entry focuses the composer only after authoritative state and viewer restoration settle, while higher focus scopes, viewers, message reading targets, and archived states retain keyboard ownership |
| **REQ-CONV-004:** Message Delivery States | ✅ Complete | Sending/sent/failed with retry; server-authoritative pending steering entries arrive live across clients and in reconnect snapshots, reconcile by message ID, compact from local storage, and surface a recoverable inconsistency if later authoritative idle evidence still lacks the message |
| **REQ-CONV-005:** Connection Status | ✅ Complete | Reconnection with backoff |
| **REQ-CONV-006:** Reconnection Data Integrity | ✅ Complete | Sequence-based deduplication |
| **REQ-CONV-007:** Agent Activity Indicators | ✅ Complete | StateBar is the sole live-activity owner; discriminated union and exhaustive labels prevent ambiguous duplicate activity surfaces |
| **REQ-CONV-008:** Cancellation | ✅ Complete | Cancel button during agent work |
| **REQ-CONV-009:** New Conversation | ⚠️ Deprecated | Replaced by REQ-CONV-015, 017, 018 |
| **REQ-CONV-010:** Responsive Layout | ✅ Complete | Viewport-specific layouts; mobile terminal status/launcher lives in the expanded state drawer; existing-conversation model, effort, and supported request speed are staged atomically in a top-layer selection dialog, with visible lock guidance while work is in flight (`StateBar`, `SelectionDialog`, `StateBar.test.tsx`, `mobileMultiPrConversation`) |
| **REQ-CONV-011:** Local Storage Schema | ✅ Complete | Namespaced keys |
| **REQ-CONV-012:** Conversation State Indicators | ✅ Complete | Part of task 561 |
| **REQ-CONV-013:** Per-Conversation Scroll Position | ⚠️ Deprecated | Removed wholesale; returning lands pinned to bottom |
| **REQ-CONV-014:** Desktop Message Readability | ✅ Complete | Part of task 561 |
| **REQ-CONV-015:** Mobile New Conversation Bottom Sheet | ✅ Complete | Part of task 561 |
| **REQ-CONV-016:** Desktop Sidebar Layout | ✅ Complete | Task 563 |
| **REQ-CONV-017:** Desktop New Conversation - Full Page Mode | ✅ Complete | Task 563 |
| **REQ-CONV-018:** Desktop New Conversation - Inline Sidebar Mode | ✅ Complete | Task 563 |
| **REQ-CONV-019:** Streaming Text Display | ✅ Complete | Task 582. `StreamingMessage` component, atomic swap in reducer |
| **REQ-CONV-020:** Navigation Persistence | ✅ Complete | Task 581. Router-level `ConversationProvider`, `lastSequenceId` in atom |
| **REQ-CONV-021:** Error Resume Affordance | ✅ Complete | User-resumable typed errors keep in-conversation recovery available, including provider prompt rejection and invalid requests; detected state-decoding failures have a separate client-only state with reload guidance and no inferred recovery actions |
| **REQ-CONV-022:** Conversation View Density | ✅ Complete | `full`/`compact` via `DensityProvider`; compact collapses tools + text previews without data loss, and compact bash cards retain identity, status, duration, and bounded output tail rather than a generic done summary |
| **REQ-CONV-023:** Conversation Navigation | ⚠️ Qualification in progress | `ConversationNav` uses persisted continuation identities for loaded and unloaded native boundary stops. Compact completed-boundary markers keep summary/provenance in native disclosures. Chapter/event exclusion, chronological mixed loaded/unloaded stops, and sole exact-ID positioning regressions pass. Desktop/mobile Chromium/WebKit fixture matrices passed; final pushed-head CI/review and live-server/physical acceptance remain gates. |
| **REQ-CONV-026:** Continuous Aggregate History | ⚠️ Qualification in progress | Global and ordinary routes share the aggregate page adapter and stable aggregate viewport identity. Global has its own lifecycle-free history API; the newest writable member remains composer-owned independently of exact historical pins. Focused regressions cover production ordinary exact-source alias routing, pending selection supersession, Global owner lifetime and warm-route resolution, outgoing live-row preservation, delayed old-ceiling cursor authority, chronological unloaded-boundary order, generated-opening readiness, and sole physical positioning with exact source-tool pulses. Joint UI/spec checks and Chromium/WebKit fixture matrices passed on the eb6 review-repair source; focused backend32/15 results belong to pre-repair f885, not the changed opening-authority DB code. Actual eb6 hosted run38328590822 subsequently passed current-source Rust execution and all seven applicable jobs; Chromium and WebKit fixture journeys cover ordinary/Global boundaries, native authored-disclosure contracts, Back/Forward, newest send, cross-member pins, and source-call click/cold reload. Final pushed-head hosted CI/review and exact-head live-server reader-anchor acceptance remain source gates; physical user acceptance is a separate release gate. Fixture evidence does not close either gate. |
| **REQ-CONV-025:** Bounded Markdown Disclosures | Implemented; joint qualification pending | Canonical native-context parser, safe renderer, streamed AST selection and scoped find integration. Exact-source full UI/TypeScript/lint, independent review, static native seams, browser bundle and real-App Chromium keyboard/find journey passed. Native Markdown hosted qualification and terminal review passed on the landed ancestry; joint history/disclosure checks remain gates. Prototype evidence is separate; inherited native HAST depth-4096 stack failure remains unresolved. |

## Opening contract integration gate

REQ-CONV-026 backend snapshots replace `latest_opening_pending` with `latest_opening`; SSE init/message/message_updated envelopes carry a nullable typed `opening` observation. Backend regression sources cover authority/content distinctions and live committed acceptance without aggregate GET. Hand-authored UI consumers, generated types, and valibot schemas intentionally remain unchanged in this backend-only unit; genuine hosted codegen and consumer reconciliation are required before integration. UI stale-pending-versus-accepted ordering and refresh-failure acceptance remain unqualified, not passed.
