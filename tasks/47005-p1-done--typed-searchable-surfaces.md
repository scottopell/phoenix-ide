# Model viewer find as typed searchable surfaces

## Problem

Viewer find currently derives search data, rendered visibility, navigation, decoration, focus restoration, and keyboard ownership through partially independent paths. Review of the initial implementation repeatedly found the same bug classes:

- Search indexes content the renderer hides.
- Rendering exposes content that search omits.
- Results are counted without a revealable target.
- Counts, active indices, and navigation derive from different arrays or reducers.
- Query state survives close and leaves stale decorations behind.
- Result-set changes reconcile stale numeric indices inconsistently.
- Focus origin is overwritten by repeated shortcuts or restored differently per surface.
- Passive content surfaces and modal scopes compete through listener order and loosely related booleans.
- Surface mode changes, such as HTML source to preview, can retain an invalid find session.
- Markdown visibility is reconstructed from React children rather than a canonical parsed model.

These are architectural failures, not isolated missing tests. Do not continue scaling the current pattern by adding surface-specific listeners, visibility predicates, index normalization, or eligibility booleans.

## Governing invariant

Every reported result must originate from a canonical semantic-content projection, carry a stable identity and typed reveal operation that makes the exact occurrence visible, and exist only inside an open find session owned by the topmost eligible keyboard layer.

Search covers the complete logical content of the active surface, including virtualized, collapsed, summarized, or otherwise temporarily hidden content. Disclosure state may hide content before navigation, but it must never make logical content unsearchable. Navigating to a hidden match mounts or expands its owner, waits for the exact occurrence to render, and highlights it. Closing a find session structurally removes all results and decorations.

## Architectural direction

### Canonical semantic-content and display projections

Separate what belongs to the logical document from how it is currently displayed, then connect them with a typed reveal operation:

```text
Canonical payload
  ├─ semantic-content projection → literal matcher → stable typed match
  └─ display/disclosure projection → renderer
                typed match → reveal operation → mount/expand → exact highlight
```

Each semantic-content node has stable identity, complete logical text, a semantic range, and a typed reveal operation. Each display node has render data, explicit disclosure state, and fragment identity shared with its semantic node.

Semantic content excludes source syntax with no user-facing reveal path: Markdown fence markers, heading punctuation, link destinations, and comments are not silently mixed into rendered-document search. Fenced code content, image alt text, tool output, sub-agent results, and collapsed message text remain searchable logical content.

Representative transcript nodes:

```ts
type TranscriptDisplayNode =
  | {
      kind: 'message-text';
      id: FragmentId;
      unitKey: string;
      text: string;
      reveal: VirtuosoFragmentTarget;
      render: MessageTextRenderModel;
    }
  | {
      kind: 'collapsed-summary';
      id: FragmentId;
      unitKey: string;
      text: string;
      reveal: VirtuosoFragmentTarget;
      render: SummaryRenderModel;
    }
  | {
      kind: 'tool-strip';
      id: FragmentId;
      unitKey: string;
      labels: readonly string[];
      reveal: VirtuosoFragmentTarget;
      render: CompactToolStripModel;
    };
```

Compact/full density, expanded/collapsed text, tool disclosure, Mermaid behavior, system-prompt expansion, hidden system rows, streaming content, latest-message expansion, and sub-agent ordering belong to the display/disclosure projection. Search remains based on complete semantic content; reveal operations consume disclosure state to expose the selected occurrence.

### Revealable matches

A match is valid only when it carries a typed adapter-owned target:

```ts
type RevealTarget =
  | PierreFileRangeTarget
  | PierreDiffRangeTarget
  | VirtuosoFragmentTarget
  | DomFragmentTarget;

type SearchMatch<T extends RevealTarget> = {
  id: MatchId;
  sourceId: FragmentId;
  sourceRange: TextRange;
  target: T;
};
```

Targets preserve every identity required for deterministic navigation: item, side, line, source range, unit key/index, fragment ID, or Markdown block ID. DOM nodes are never match identity. Adapters return an explicit reveal outcome rather than silently failing.

### Unified find session

Replace surface-local combinations of `useViewerFind`, local query/index state, projection counts, opener refs, and decoration gates with one typed state machine:

```ts
type FindSession<T extends RevealTarget> =
  | { status: 'closed' }
  | {
      status: 'open';
      query: string;
      results: readonly SearchMatch<T>[];
      activeMatchId: MatchId | null;
      focusOrigin: FocusOrigin;
    };
```

Required properties:

- Closed sessions cannot contain query results, active matches, or decorations.
- Empty queries produce no results.
- Count and navigation derive only from `results`.
- Active identity is a stable `MatchId`, not merely an array index.
- Projection changes preserve the same active `MatchId` when it remains present; otherwise choose the nearest match in document order deterministically.
- Opening an already-open session issues a refocus/select effect without replacing `focusOrigin`.
- Closing clears results/decorations and restores the original valid focus origin.
- Surface identity/mode changes close or replace incompatible sessions structurally.
- Streaming updates may update results while open without subscribing parent rendering paths while closed.

Effects such as scroll, wait-for-mount, exact reveal, decoration, and focus restoration are explicit commands produced by state transitions rather than ad hoc component effects.

### Central keyboard routing

Separate modal interaction ownership from passive command eligibility. Components register typed keyboard capabilities with a central router rather than installing competing global listeners.

Representative layers:

```ts
type KeyboardLayer =
  | { kind: 'modal'; id: ScopeId; commands: CommandSet }
  | { kind: 'viewer'; id: ScopeId; commands: CommandSet }
  | { kind: 'passive-content'; id: ScopeId; commands: CommandSet };
```

Routing rules:

- The topmost modal receives first refusal.
- The topmost viewer receives viewer commands.
- A passive transcript may claim find only when no viewer/modal obscures it.
- Global Escape runs only when no higher layer handles Escape.
- Editable-target policy is centralized.
- A find input handles repeated find without redispatching an opener event.
- Dialog isolation is represented by layer registration, not a `dialogOpen` boolean threaded into unrelated hooks.
- Listener registration order cannot change command ownership.

### Markdown display model

Parse Markdown into stable typed display blocks before rendering:

```ts
type MarkdownDisplayBlock = {
  id: BlockId;
  sourceRange: TextRange;
  searchableText: string;
  render: MarkdownRenderNode;
  reveal: DomFragmentTarget;
};
```

The renderer and matcher consume the same blocks. Inline code, emphasis, links, fenced code, Mermaid diagrams, tables, and nested children must not depend on recovering strings from React children or scraping rendered DOM.

### Explicit surface capabilities

Represent searchable and ineligible modes as discriminated variants rather than inferred boolean combinations. For example, `html-source` carries a searchable projection while `html-preview` cannot carry a find capability. Message sources cannot acquire file-only targets, and non-text surfaces cannot construct searchable sessions.

## Scope

Apply the architecture across:

1. Pierre file/code viewer.
2. Pierre unified and split diff viewer, including commit log.
3. React Virtuoso conversation transcript and active streaming text.
4. Task approval plan.
5. Markdown, HTML source, and large-text document bodies.
6. Conversation-message side viewer as it moves through task 47004's unified document viewer.

Do not wait for upstream exact-range Pierre decorations. Phoenix targets remain typed and exact; the Pierre adapter may report a line-level decoration capability until task 47003 lands upstream support.

## Implementation sequence

1. Inventory all global keyboard listeners, focus scopes, viewer modes, search projections, and renderer visibility predicates.
2. Add normative requirements/Allium where the cross-surface lifecycle and keyboard-layer transitions need precise state behavior.
3. Introduce branded IDs, reveal targets, display nodes, and the closed/open find-session state machine with property tests.
4. Introduce central keyboard layers and migrate find/Escape ownership; remove surface-level global listeners.
5. Migrate transcript projection first because it has the richest visibility/disclosure model and streaming isolation constraints.
6. Migrate task/Markdown through a parsed display-block model.
7. Migrate diff and file projections, preserving Pierre note, scroll-restoration, and typed navigation behavior.
8. Expose the model to task 47004's source-aware document viewer and migrate conversation-message viewing.
9. Delete parallel search-only extractors, duplicated visibility predicates, compatibility booleans, and obsolete focus/Escape paths.

Each migration must remain shippable and remove its superseded representation in the same unit; do not leave canonical and legacy projections active in parallel.

## Verification

### State model

- Property tests cover open/refocus/query/next/previous/close, wraparound, empty results, result-set growth/shrink/reorder, active identity preservation, surface replacement, and focus restoration.
- Invalid states such as closed-with-results, active-without-result, and ineligible-surface-with-session are unrepresentable.
- Every result has a reveal target and every reveal target maps back to exactly one display node.

### Projection/render parity

- Golden tests assert searchable text and visual order from the same display projection used by each renderer.
- Compact/full transcript, disclosure, Mermaid, tool details, system prompt, skills, sub-agents, streaming, and latest-message behavior cannot diverge between render and search.
- Markdown tests cover inline/nested formatting, fenced code, diagrams, tables, and source-range mapping.
- Diff tests cover insertions/deletions shifting context alignment, unified/split sides, commit log, committed/uncommitted identity, and off-screen files.

### Keyboard and focus

- Layer tests cover passive transcript, viewer overlay, annotation dialog, confirmation dialog, find input, editable controls, and global Escape.
- Repeated `Cmd/Ctrl+F` refocuses/selects without replacing the original focus origin.
- Escape closes only the topmost interactive layer and restores focus deterministically.
- Unmount and surface-mode transitions release every keyboard registration.

### Browser QA

- Real-browser scenarios verify native find suppression, focus selection/restoration, initially unmounted navigation, virtualization remount, streaming append, disclosure changes, dialog priority, HTML mode switching, and source-specific decoration behavior.

## Acceptance criteria

- Search consumes canonical semantic content and rendering consumes a related typed display/disclosure projection with shared stable fragment identity.
- Every counted result is revealable through its typed operation; navigation mounts or expands hidden content and highlights the exact occurrence.
- One find-session state machine owns query, results, active identity, focus origin, reconciliation, and decoration lifecycle.
- One keyboard router owns topmost command dispatch; surface-local global find/Escape listeners are removed.
- Surface eligibility and mode transitions are encoded by discriminated capabilities rather than boolean combinations.
- Parallel visibility predicates and search-only reconstructions are deleted.
- Existing viewer note, navigation, scroll-restoration, tail-follow, streaming-isolation, and annotation behavior remains covered.
- Task 47004 consumes this foundation rather than reproducing the current file/message divergence in a new wrapper.

## Implementation progress

The foundation and first surface migrations are established:

- A pure typed find-session state machine owns structural closed/open state, stable `MatchId`, reconciliation, focus origin, wraparound navigation, and explicit reveal/focus/decoration commands.
- A React adapter delivers command batches exactly once and does not re-reveal surviving active matches during projection updates.
- Search projection matches carry semantic source identity separately from mutable navigation coordinates.
- Diff and file viewers use typed sessions and preserve active semantic identity when line insertions move reveal targets.
- Keyboard shortcuts route through one provider-owned layered router rather than competing global listeners.
- Assistant transcript text uses shared typed semantic/display fragments: compact-hidden content remains searchable, navigation expands it, and the renderer highlights the exact active occurrence.

Remaining migration order:

1. Define a typed capability matrix for heterogeneous tool renderers, then migrate tool semantic/display fragments incrementally by renderer family rather than as one raw-input/result adapter.
2. Migrate sub-agent cards with renderer-owned disclosure and exact reveal targets.
3. Parse task/file Markdown into semantic display blocks shared by rendering, matching, and reveal.
4. Migrate the transcript itself from numeric local state to the typed session and remove legacy row-level highlight paths.
5. Migrate task approval, remove `useViewerFind`/`viewerFindReducer`, and complete the source-aware document-viewer integration with task 47004.

## Relationship to other tasks

- **Task 47004** — source-aware side-panel document viewer consolidation should depend on or be sequenced after this foundation. Its unified document viewer consumes typed display projections and the shared find session; it must not copy current file/message find composition.
- **Task 47003** — upstream Pierre read-only exact-range decorations improves visual fidelity but does not block typed Phoenix search/reveal architecture.
