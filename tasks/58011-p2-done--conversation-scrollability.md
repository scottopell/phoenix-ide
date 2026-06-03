Make long conversations skimmable. Today the scrollback is dominated by tool
calls; the "significant" content — your prompts and the substantial assistant
prose (findings, plans, summaries) — is buried.

Two independently shippable features plus a spec update, from a design
discussion.

## Design principle: one slot, one role

The existing breadcrumb bar (`#breadcrumb-bar`, fed by `atom.breadcrumbs` via
`breadcrumbFromPhase`) sits at conversation-level position (a sticky bar
spanning the view) but carries turn-level content (this turn's tools, wiped on
every user message). That mismatch is why it's a poor nav tool.

We do NOT resolve this by time-sharing the slot (nav when idle, live trail
when streaming) — a slot that changes role between cold load and streaming
blurs two visually-similar-but-semantically-distinct things. Instead each
concern gets a fixed home:

- **Horizontal bar slot → dedicated to Conversation Nav (Feature B), always.**
  Same role on cold load and while streaming. The in-flight turn just appears
  as the newest chapter as its prose streams in; the bar's job never changes.
- **Live "what's the agent doing right now" → stays in the StateBar**, which
  already owns it (REQ-CONV-007 pulsing dot + `isAgentWorking` state label).
  Not in the nav slot.
- **Per-turn tool detail → inline in the message list (Feature A)**, never in
  the bar.

This retires the breadcrumb's current turn-trail role in the top slot; that
information is already covered by the StateBar.

## Feature A — Compact density mode (collapse per turn)

A persisted view-density setting (Full / Compact) in `SettingsDropdown`.
Compact mode:

- Each `agent_turn`'s tool calls collapse into a single inline mini-pill strip
  (`AI -> bash -> bash`), reusing the existing pill styling + type colors
  (tool = `--accent-purple`, sub-agents = `--accent-yellow`). Click a pill ->
  expand that turn / jump to the tool.
- Assistant text blocks under the significance threshold collapse to a faded
  one-liner; substantial prose stays full. User messages always render full.
- Nothing is destroyed — every collapsed element expands on click. Full mode
  is today's behavior.

"Significant" = assistant `text.length` over a named threshold (start ~280
chars, tune later). Because Compact collapses-never-hides, a false negative
just means a faded expandable one-liner — safe.

Implementation:
- Setting via `useLocalStorage<'full'|'compact'>('phoenix-conv-density',
  'full')`, exposed through a small context (mirror `useTheme`) so
  `MessageComponents` reads it without prop-drilling. Add a control row to
  `SettingsDropdown`.
- Purely presentational: `buildRenderUnits` stays the single source of truth
  for which messages render and how they group. Density only changes how an
  `agent_turn` and short text blocks paint — changes live in
  `MessageComponents` (`AgentMessage` / `ToolUseBlock`), not `renderUnits.ts`.
- Build the collapsed strip from the turn's `ContentBlock[]` tool_use blocks +
  `toolResultsByUseId`, not from phase state.

## Feature B — Conversation navigation (horizontal chapter strip)

The dedicated bar's items are whole-conversation chapters: user messages PLUS
assistant text blocks over the significance threshold. Each item: a
type-styled pill, label = truncated prompt / first line of prose, click ->
scroll to that message, with scroll-spy highlighting the chapter currently in
view.

CRITICAL RISK — virtualization. `MessageList` renders through react-virtuoso
(`virtuosoRef`, `scrollToIndex`). The existing breadcrumb jump uses
`document.querySelector('[data-sequence-id=...]').scrollIntoView()`, which only
works because the current turn is near the viewport — an off-screen
virtualized row is NOT in the DOM, so the query returns null. Feature B must
jump via `virtuosoRef.scrollToIndex({ index })` keyed by the target's
render-unit index (the `HistoricalUnit` position), and apply the
`.breadcrumb-highlight` pulse AFTER the row mounts (post-scroll callback / row
effect), since the element doesn't exist at click time.

Implementation:
- New pure selector over `historicalUnits` (sibling to `buildRenderUnits`)
  emitting `{ unitIndex, kind: 'prompt'|'prose', label, sequenceId }[]` — keeps
  classification in one testable place, consistent with the
  `messagelist-render-units` "one transform decides" principle.
- Scroll-spy via Virtuoso `rangeChanged` (or IntersectionObserver on rendered
  `data-sequence-id` nodes) to compute the active chapter.
- Lift a `scrollToUnitIndex(index)` handle out of `MessageList` (it owns
  `virtuosoRef`) to the nav bar; pulse the target after scroll settles.
- Long conversations: the bar is already horizontally scrollable and
  auto-scrolls to the active end, so many chapters degrade gracefully. A
  vertical outline rail is the fallback shape if it gets crowded — deferred.

## Suggested sequencing

1. Extract a shared `<PillStrip>` from `BreadcrumbBar` (pure refactor, no
   behavior change) — the visual primitive A and B both need.
2. Feature A (Compact density) — highest leverage, low risk; per-turn tool
   collapse is what's burying the prose.
3. Feature B (Conversation nav) — item selector + virtuoso-based jump +
   scroll-spy. Higher risk (virtualization), so it follows A.

Each is independently shippable.

## Spec note — specs/conversation-ui/ (normative)

- REQ-CONV-007 (Agent Activity Indicators): confirm the live "agent working"
  indicator is owned by the StateBar; remove any implication that the top
  breadcrumb slot is a conversation nav aid or a turn trail. The top slot is
  Conversation Nav.
- New requirement — collapsed-turn density (Feature A): a view-density
  preference that collapses a completed turn's tool activity into an inline
  pill strip and short assistant prose into expandable one-liners, full-fidelity
  expand-on-click, no data loss. Define the significance threshold constant.
- New requirement — conversation navigation (Feature B): a persistent
  whole-conversation chapter strip over significant messages (prompts + key
  prose) with click-to-jump (via virtualized scroll-to-index) and scroll-spy
  active tracking, with a single fixed role.
- design.md: document the one-slot-one-role split (nav slot vs StateBar live
  indicator vs inline per-turn detail), the shared `<PillStrip>` primitive, and
  the virtualization constraint (jump by render-unit index, not querySelector,
  because off-screen rows are unmounted). Keep it timeless — no task/PR refs.
- Consider an Allium spec for Feature B (scroll-spy state + jump lifecycle);
  Feature A is presentational, spEARS-only is sufficient.

## Open / deferred

- Exact significance threshold value (start ~280 chars).
- Whether Compact collapses `think` asides further (they already self-collapse).
- Vertical outline rail as an alternative nav shape — deferred.
