# Conversation Scrollability — Plan & Spec Note

Goal: make long conversations skimmable. Today the scrollback is dominated by
tool calls; the "significant" content — your prompts and the substantial
assistant prose (findings, plans, summaries) — is buried.

This plan splits the work into **two independent features** plus a **spec
update**, derived from a design discussion. They share visual/interaction
vocabulary with the existing breadcrumb bar but are scoped differently.

## The reframe: scope the breadcrumb correctly

The current breadcrumb bar (`#breadcrumb-bar`, fed by `atom.breadcrumbs` via
`breadcrumbFromPhase`) sits at **conversation-level position** (a sticky bar
spanning the whole view) but carries **turn-level content** (this turn's
tools, wiped on every user message). That mismatch is why it's a poor
navigation tool — you rarely want to jump to a specific tool within the last
turn. We resolve it by splitting the concept by scope:

| Role | Scope | Source | Lifetime | Job |
|---|---|---|---|---|
| Live activity trail (keep as-is) | current in-flight turn | `breadcrumbFromPhase` (phase state) | resets each user message | "what is the agent doing right now" |
| Collapsed-turn strip (Feature A) | one past turn | that turn's persisted tool calls | persistent, inline | "what did this turn do" + expand |
| Conversation nav (Feature B) | whole conversation | persisted messages (`buildRenderUnits`) | persistent | "jump to a significant message" |

The live trail keeps its current job for the in-flight turn; once a turn
completes, the same pill-strip visual freezes into Feature A's inline
collapsed representation. Feature B reuses the breadcrumb's *interaction*
vocabulary (pill styling, `.breadcrumb-highlight` pulse) but operates across
all turns.

---

## Feature A — Compact density mode (collapse per turn)

A persisted **view-density** setting (Full / Compact) in `SettingsDropdown`.
In Compact mode:

- Each `agent_turn` render unit's tool calls collapse into a single inline
  **mini-breadcrumb strip** (`AI → bash → bash`), reusing the existing pill
  styling + type colors (tool = `--accent-purple`, sub-agents =
  `--accent-yellow`). Click a pill → expand that turn (or jump to the tool).
- Assistant text blocks under a length threshold (the "significant" cutoff —
  see below) collapse to a faded one-liner; substantial prose stays full.
- User messages always render full.
- **Nothing is destroyed** — every collapsed element expands on click. Full
  mode is today's behavior.

### "Significant" classification (length threshold)

An assistant text block is significant if its `text.length` exceeds a
threshold (start ~280 chars / a few sentences; make it a named constant,
tune later). Short blocks ("Let me check that file.") are filler. Risk is
false negatives, which is why Compact **collapses, never hides** — the faded
one-liner is always present and expandable.

### Implementation sketch (Feature A)

- **Setting:** `useLocalStorage<'full' | 'compact'>('phoenix-conv-density',
  'full')`, exposed via a small context (mirror `useTheme`) so
  `MessageComponents` can read it without prop-drilling. Add a control row to
  `SettingsDropdown`.
- **Rendering:** Feature A is purely presentational. `buildRenderUnits` stays
  as the single source of truth for *which* messages render and how they
  group; density only changes *how* an `agent_turn` and short text blocks
  paint. So the changes live in `MessageComponents` (`AgentMessage` /
  `ToolUseBlock`), not `renderUnits.ts`.
- **Collapsed strip:** factor the pill rendering out of `BreadcrumbBar` into a
  shared `<PillStrip>` so the live trail (Feature B/keep) and the inline
  collapsed-turn strip share one component. Build its items from the turn's
  `ContentBlock[]` (tool_use blocks) + `toolResultsByUseId` rather than from
  phase state.

---

## Feature B — Conversation-level navigation (horizontal chapter strip)

Reuse the existing horizontal sticky bar slot, but its items become
**whole-conversation chapters**: your user messages **plus** assistant text
blocks over the significance threshold. Each item:

- Label = truncated prompt / first line of the prose.
- Type-styled pill (user vs assistant accent).
- Click → scroll to that message. **Scroll-spy** highlights the item whose
  message is currently in view.

### Critical technical risk: virtualization

`MessageList` renders through **react-virtuoso** (`virtuosoRef`,
`scrollToIndex`). The existing breadcrumb jump uses
`document.querySelector('[data-sequence-id=...]').scrollIntoView()`, which
only works because the current turn is near the viewport — an off-screen
virtualized row is **not in the DOM**, so the query returns null.

Feature B must therefore jump via **`virtuosoRef.scrollToIndex({ index })`**
keyed by the target's **render-unit index** (the `HistoricalUnit` position),
not by `data-sequence-id`. Apply the `.breadcrumb-highlight` pulse *after* the
row mounts (e.g. in a `rangeChanged`/post-scroll callback or an effect on the
newly mounted row), since the element doesn't exist at click time.

### Implementation sketch (Feature B)

- **Item derivation:** a new pure selector over `historicalUnits` (sibling to
  `buildRenderUnits`, or a follow-on transform) emitting
  `{ unitIndex, kind: 'prompt' | 'prose', label, sequenceId }[]`. Keeps
  classification in one testable place, consistent with the
  `messagelist-render-units` spec's "one transform decides" principle.
- **Scroll-spy:** subscribe to Virtuoso's `rangeChanged` (or an
  IntersectionObserver on rendered `data-sequence-id` nodes) to compute the
  active chapter; highlight that pill.
- **Jump:** lift a `scrollToUnitIndex(index)` handle out of `MessageList`
  (it already owns `virtuosoRef`) and pass it to the nav bar, or route the
  click through an existing ref/context. After scroll settles, pulse the
  target row.
- **Long conversations:** the bar is already horizontally scrollable +
  auto-scrolls to the active end, so many chapters degrade gracefully. If
  density becomes a problem, a vertical outline rail is the fallback shape
  (out of scope for v1).

---

## Suggested sequencing

1. **Shared `<PillStrip>`** — extract from `BreadcrumbBar` with no behavior
   change (pure refactor, keeps the live trail working). Lands the shared
   visual primitive A and B both need.
2. **Feature A (Compact density)** — setting + context + inline collapse.
   Highest leverage: per-turn tool collapse is what's burying the prose.
3. **Feature B (Conversation nav)** — item selector + virtuoso-based jump +
   scroll-spy. Higher risk (virtualization), so it follows A.

Each is independently shippable behind the density setting / its own toggle.

---

## Spec note — `specs/conversation-ui/`

These are **normative** changes (both spEARS and any Allium are authoritative),
so the plan includes spec edits, not just code.

- **REQ-CONV-007 (Agent Activity Indicators)** currently frames the breadcrumb
  as a conversation-level tool trail. Re-scope its prose to the **in-flight
  turn's live activity trail** (its real job), and stop implying it is a
  conversation navigation aid.
- **New requirement — collapsed-turn density (Feature A):** a view-density
  preference that collapses a completed turn's tool activity into an inline
  pill strip and short assistant prose into expandable one-liners, with
  full-fidelity expand-on-click and no data loss. Define the significance
  threshold as the named cutoff.
- **New requirement — conversation navigation (Feature B):** a persistent
  whole-conversation chapter strip over significant messages (prompts + key
  prose) with click-to-jump (via virtualized scroll-to-index) and scroll-spy
  active tracking.
- **`design.md`:** document the three-way scope split (live trail / collapsed
  strip / conversation nav), the shared `<PillStrip>` primitive, and the
  virtualization constraint (jump by render-unit index, not
  `querySelector`, because off-screen rows are unmounted). Keep it timeless —
  describe the design as standing fact, no task/PR references.
- Consider whether Feature B's classification + jump warrant an **Allium**
  spec (it has scroll-spy state + a jump lifecycle); Feature A is presentational
  and spEARS-only is sufficient.

## Open / deferred decisions

- Exact significance threshold value (start ~280 chars, tune from real
  conversations).
- Whether Compact also collapses `think` asides further (they already
  self-collapse).
- Vertical outline rail as an alternative nav shape if the horizontal strip
  gets crowded — explicitly deferred.
