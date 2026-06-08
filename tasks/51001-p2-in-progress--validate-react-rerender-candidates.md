# Validate and Fix React Unnecessary Re-render Candidates

## Goal

Audit the identified React re-render candidates with measurement first, fix only candidates that are empirically valid, and spin out follow-up tasks for changes that are too large to land safely in one pass.

This task is intentionally scientific: every candidate starts as a hypothesis, not a bug. A candidate becomes actionable only after profiling shows avoidable renders, avoidable work, or user-visible latency attributable to the suspected pattern.

## Validation method

For each candidate:

1. Define a minimal user scenario that exercises the suspected re-render path.
2. Capture a baseline with raw per-run samples, not averages only.
   - Prefer the existing Phoenix browser profiling harness where applicable.
   - Use CPU throttling to reduce host-noise dominance.
   - Include enough warmup/runs to compare distributions.
3. Add targeted render instrumentation only where needed.
   - Use React Profiler / existing `RenderProfiler` wrappers when practical.
   - Count component body executions for memoization hypotheses.
   - Count effect registrations for listener-churn hypotheses.
4. State an acceptance threshold before changing code.
   - Example: unrelated tree rows do not re-render when expanding one folder.
   - Example: drag frame work is confined to the resized pane, not the whole conversation page.
   - Example: a provider update only re-renders consumers of the changed slice.
5. Implement the smallest correct-by-construction fix.
   - Prefer structural isolation over discipline: split contexts, selector hooks, local wrapper components, or external-store slices rather than comments saying “do not subscribe broadly.”
   - Avoid parallel representations of the same state.
   - Avoid runtime conventions where types can encode the boundary.
6. Re-run the same scenario and compare raw samples.
7. Keep the fix only if the validation shows a meaningful improvement without regressions.
8. If the candidate is valid but large/risky, create a follow-up task with the evidence and leave the code unchanged except for any low-risk instrumentation removal.

## Candidate inventory

### C1. FileTree row memoization defeated by broad context

Files:
- `ui/src/components/FileExplorer/FileTree.tsx`

Hypothesis:
- `FileTreeItem` is wrapped in `memo`, but every item reads `TreeCollectionsCtx`, whose value changes when `childItems`, `expandedPaths`, `loadingPaths`, or `activeFile` changes.
- Expanding or loading one folder may re-render every mounted tree item.

Validation scenario:
- Open a conversation with a populated file tree.
- Expand/collapse one nested folder.
- Measure `FileTreeItem` render counts for affected branch vs unrelated visible branches.

Likely correct-by-construction fix:
- Remove broad collection context from row components where possible.
- Pass row-local primitives and child arrays as props, or split into selector-like stores so unrelated rows cannot observe unrelated path changes.

Escalate if:
- Refactoring recursive tree rendering requires broad architectural changes.

### C2. ConversationPage unstable function/object props propagate parent churn

Files:
- `ui/src/pages/ConversationPage.tsx`
- dependent children: `ConnectedInputArea`, `ConnectedStateBar`, `WorkControlBar`, `ConversationNavStack`

Hypothesis:
- Unmemoized handlers and inline object/function props create fresh identities on every `ConversationPage` render, weakening memo boundaries.
- Examples include `handleSend`, `handleCancel`, `handleTriggerContinuation`, `handleUpgradeModel`, inline `onSendMessage`, inline `onOpenFiles`, and inline `continuation` object.

Validation scenario:
- Trigger a page-level state update unrelated to message/input/statebar content, such as toggling context banner or opening/closing auxiliary UI.
- Count renders in `ConnectedInputArea`, `StateBar`, `WorkControlBar`, and `ConversationNavStack`.

Likely correct-by-construction fix:
- Convert handlers to stable `useCallback` values with primitive dependencies.
- Memoize structured props such as continuation configuration.
- Avoid creating render-time closures for hot child boundaries.

Escalate if:
- Stabilizing handlers requires changing async send/cancel semantics or queue ownership.

### C3. Resizable pane pointermove state re-renders too much UI

Files:
- `ui/src/hooks/useResizablePane.ts`
- `ui/src/pages/ConversationPage.tsx`
- `ui/src/components/DesktopLayout.tsx`

Hypothesis:
- `useResizablePane` calls React state setters on every pointer move.
- When pane state is owned by `ConversationPage`, dragging terminal/viewer dividers can re-render the whole conversation subtree at pointer frequency.

Validation scenario:
- Profile dragging terminal and viewer dividers for a few seconds in a populated conversation.
- Count `ConversationPage`, `MessageList`, `StateBar`, `InputArea`, and `TerminalPanel` render executions during drag.

Likely correct-by-construction fix:
- Move pane state into smaller boundary components, or use a ref/CSS-variable live-drag path with a committed React state update at drag end.
- Ensure the type/API distinguishes live transient drag state from committed layout state.

Escalate if:
- A generalized pane-layout redesign is needed.

### C4. Inline `max` functions cause `useResizablePane` callback/effect churn

Files:
- `ui/src/hooks/useResizablePane.ts`
- `ui/src/pages/ConversationPage.tsx`
- `ui/src/components/DesktopLayout.tsx`

Hypothesis:
- Inline `max={() => ...}` functions recreate `resolveMax`, `clamp`, `startDrag`, and resize effects on parent render.

Validation scenario:
- Instrument resize effect attach/detach counts across unrelated parent renders.
- Verify whether listeners re-register without pane option semantic changes.

Likely correct-by-construction fix:
- Memoize max callbacks at call sites or redesign `useResizablePane` options to accept stable typed max policies.

Escalate if:
- The pane hook API needs broader migration.

### C5. ChainBlock receives global menu/keyboard/active state

Files:
- `ui/src/components/ConversationList.tsx`

Hypothesis:
- Memoized `ChainBlock` receives `expandedRowId`, `keyboardSelectedId`, and `activeSlug`, so row/menu/keyboard changes can re-render every chain block.

Validation scenario:
- Seed many chains in the sidebar.
- Move keyboard selection or open one row menu.
- Count re-renders for unrelated chain blocks and member rows.

Likely correct-by-construction fix:
- Derive per-chain props before rendering: whether this chain contains the active row, selected row, or expanded row.
- Split chain header from member list if only one part needs the global signal.

Escalate if:
- Sidebar grouping needs a larger data-model change.

### C6. ReviewNotesContext broadcasts every note mutation to all note consumers

Files:
- `ui/src/contexts/ReviewNotesContext.tsx`
- `ui/src/components/viewer/useFileReviewNotes.ts`
- `ui/src/components/viewer/useDiffReviewNotes.ts`

Hypothesis:
- File and diff viewers subscribe to the entire notes context. Adding a note in one scope re-renders consumers in other scopes and refilters the entire pile.

Validation scenario:
- Open a file viewer and diff viewer path where possible.
- Add/remove file notes and diff notes.
- Count renders and filtering work for unrelated note scopes.

Likely correct-by-construction fix:
- Split command context from data subscriptions.
- Add selector hooks such as `useFileReviewNotesData(path)` and `useDiffReviewNotesData()` so unrelated note families cannot observe each other.

Escalate if:
- The shared “send entire pile” behavior requires a deeper typed notes-store redesign.

### C7. FocusScopeProvider broadcasts command-only consumers

Files:
- `ui/src/hooks/useFocusScope.tsx`

Hypothesis:
- `useRegisterFocusScope` consumers only need `pushScope` and `popScope`, but subscribe to a context value that also changes with `activeScope`, `hasActiveScope`, and `isActiveScope`.

Validation scenario:
- Open/close file viewer, diff viewer, question panel, command palette, and shortcut help.
- Count re-renders in command-only focus-scope consumers.

Likely correct-by-construction fix:
- Split focus-scope commands from focus-scope state into separate contexts.
- Keep state consumers explicitly typed as state consumers.

Escalate if:
- Focus scope semantics need a broader lifecycle spec update.

### C8. ThemeProvider inline context value

Files:
- `ui/src/components/ThemeProvider.tsx`

Hypothesis:
- `ThemeProvider` creates `{ theme, toggleTheme }` inline and `toggleTheme` is not memoized, causing avoidable broadcasts if the provider re-renders without a theme change.

Validation scenario:
- Force an app-level provider re-render that does not change theme.
- Count theme consumer renders.

Likely correct-by-construction fix:
- `useCallback` for `toggleTheme` and `useMemo` for provider value.

Escalate if:
- None expected; this is likely small.

### C9. ViewerSlotContext is broad

Files:
- `ui/src/contexts/ViewerSlotContext.tsx`
- `ui/src/components/WorkActions.tsx`
- `ui/src/components/FileExplorer/FileExplorerContext.tsx`
- `ui/src/pages/ConversationPage.tsx`

Hypothesis:
- Consumers that need only commands or only `browserSessionActive` subscribe to the full slot object.
- URL/viewer changes may re-render command-only consumers.

Validation scenario:
- Open/close prose, diff, and browser viewers.
- Count renders in work actions, file explorer provider, and conversation page sections that do not need the changed slot payload.

Likely correct-by-construction fix:
- Split viewer-slot state and commands, or add typed selector hooks for `slotKind`, `browserSessionActive`, and commands.

Escalate if:
- Viewer slot spec/API needs to be revised.

### C10. CommandPalette rebuilds action state from full conversations array

Files:
- `ui/src/components/CommandPalette/CommandPalette.tsx`

Hypothesis:
- `actions` depends on the full `conversations` array, so sidebar/store updates can rebuild actions and rebind effects even when the palette-visible command set is semantically unchanged.

Validation scenario:
- Keep command palette closed and then open while conversation polling/SSE updates active conversation metadata.
- Count action rebuilds, shortcut listener re-registrations, and search effect restarts.

Likely correct-by-construction fix:
- Store conversations in a ref for action execution, and depend on narrower primitives for action shape.
- Preserve correctness for archive-current by reading latest data at execution time.

Escalate if:
- Command source/action ownership needs redesign.

### C11. ConversationNavStack lacks a memo boundary

Files:
- `ui/src/components/ConversationNavStack.tsx`

Hypothesis:
- Even if `MessageList` bails out, parent churn still re-renders `ConversationNavStack` and `ConversationNav`.

Validation scenario:
- Trigger parent-only state updates with unchanged message/nav inputs.
- Count renders in `ConversationNavStack`, `ConversationNav`, and `MessageList`.

Likely correct-by-construction fix:
- Stabilize upstream props first, then wrap `ConversationNavStack` in `memo`.

Escalate if:
- Nav state should be moved closer to message-list virtualization rather than memoized.

### C12. StateBar receives fresh parent props despite heartbeat isolation

Files:
- `ui/src/pages/ConversationPage.tsx`
- `ui/src/components/StateBar.tsx`

Hypothesis:
- Heartbeat updates are isolated via a ref, but parent re-renders still propagate fresh `continuation` and `onOpenFiles` props into `ConnectedStateBar`/`StateBar`.

Validation scenario:
- Trigger unrelated parent state changes while connection/phase/model props are stable.
- Count `ConnectedStateBar` and `StateBar` renders.

Likely correct-by-construction fix:
- Memoize `continuation` and callbacks; consider memoizing presentational `StateBar` if props are stable.

Escalate if:
- `StateBar` should be split into independent subscription-driven subcomponents.

### C13. WorkViewerActions subscribes to entire viewer slot

Files:
- `ui/src/components/WorkActions.tsx`
- `ui/src/contexts/ViewerSlotContext.tsx`

Hypothesis:
- `WorkViewerActions` needs only specific viewer-slot fields/commands, but `useViewerSlot()` subscribes to the full provider value.

Validation scenario:
- Change prose file selection while work actions are visible.
- Count `WorkControlBar` / `WorkViewerActions` renders.

Likely correct-by-construction fix:
- Reuse the narrower viewer-slot selectors from C9.

Escalate if:
- Same as C9.

### C14. FileExplorerPanel passes an unstable `handleFileSelect` into FileTree

Files:
- `ui/src/components/FileExplorer/FileExplorerPanel.tsx`

Hypothesis:
- `handleFileSelect` is recreated on every `FileExplorerPanel` render. If `FileTree` becomes memoized or expensive, this defeats the boundary.

Validation scenario:
- Render file explorer, trigger panel-only state changes such as selecting skill/task views or toggling skills panel.
- Count `FileTree` renders with unchanged tree inputs.

Likely correct-by-construction fix:
- Wrap `handleFileSelect` in `useCallback`.

Escalate if:
- None expected; small only if validated or bundled with C1.

### C15. MessageList creates Virtuoso slot component types inside render memo blocks

Files:
- `ui/src/components/MessageList.tsx`

Hypothesis:
- `SystemPromptHeaderSlot` and `EmptyPlaceholder` are component types created inside `useMemo`. Changes to system prompt expanded state can create a new header component type and cause Virtuoso slot remount/churn.

Validation scenario:
- Toggle system prompt expansion and inspect Virtuoso header remount/render behavior.
- Measure whether this causes list layout churn beyond the header itself.

Likely correct-by-construction fix:
- Use stable component types with data passed through props/context accepted by Virtuoso, if supported.

Escalate if:
- Virtuoso API constraints require a larger list rendering change.

## Deliverables

For each candidate, produce one of:

- `validated-fixed`: evidence, patch, before/after raw measurements, and regression tests where practical.
- `validated-follow-up`: evidence plus a new task because the fix is too large/risky for this task.
- `not-validated`: evidence showing no meaningful avoidable re-render or no meaningful user impact.

At the end, summarize:

- Which candidates were fixed.
- Which candidates were spun out into follow-up tasks.
- Which candidates were rejected by measurement.
- Any instrumentation added and removed.

## Guardrails

- Do not cargo-cult `useMemo`/`useCallback`; only add them when the measured boundary benefits or when they are required for a structural selector/context split.
- Prefer structural data-flow fixes over comments or conventions.
- Do not introduce duplicate state representations.
- Keep provider/context APIs typed so consumers cannot accidentally subscribe to state they do not need.
- Preserve existing specs and tests. If a spec governs a touched behavior, read it before changing code.
