# Redesign mobile conversations list around current work, chains, and PR relevance

## Context

The mobile conversations list is functional but has drifted behind the desktop/sidebar experience. It currently presents conversation rows like dense inventory cards: long slugs, state dots, chain position/latest/mode/PR badges, created→updated timestamps, message counts, project/model/cwd, and multiple action menus all compete at the same level.

Recent chain and PR improvements are not consistently reflected on mobile:

- Completed non-latest chain minimization exists only for `sidebarMode`, so the mobile full-page list still gives historical chain members too much visual weight.
- PR badges exist via `cached_pr`, and rows already suppress them on non-latest chain members, but the mobile layout does not make PR relevance easy to scan.
- Chain grouping exists, but mobile does not clearly separate chain navigation from opening the latest conversation.
- The mobile header gives `Archived`, auth status, and `+ New` similar visual weight, making the top of the page feel crowded.

The redesign should treat the mobile list as a decision surface, not an exhaustive record dump. It should answer: what can I resume, what needs attention, which chain member is current, and whether current work has PR relevance.

## Product decisions

Settled decisions for this task:

1. Collapsed chain cards should have an explicit chain title/button for opening the chain page. Opening the latest conversation must be a separate, clear tap target.
2. Chains should default to a compact mobile presentation, with latest/current work visible and historical completed members minimized. Active chain members must still be revealed.
3. Default mobile context should show a compact project/folder/work-context signal, not the full cwd.
4. PR badges should remain clickable on desktop, but in the first mobile redesign they should be non-clickable visual relevance badges to avoid stealing row taps.

## Goals

- Make mobile rows scannable at phone widths.
- Make current/latest chain work visually distinct from history.
- Surface PR relevance for current/latest work without badge soup.
- Preserve enough expert disambiguation to avoid false opens.
- Keep destructive and chain-scoped actions clearly scoped.
- Avoid a full IA rewrite: reuse existing conversation, chain, and PR data where possible.

## Information architecture

### Mobile page header

Redesign the mobile full-page header toward a compact command area:

```text
Conversations                         + New
Active · Archived 8                   Auth ✓
```

Requirements:

- `+ New` remains the primary action.
- Active/Archived is secondary navigation with archived count when available.
- Auth status remains visible but should not dominate unless intervention is needed.
- Header should consume less vertical space than the current large button row.

### Standalone conversation row

Default mobile shape:

```text
● conversation-slug                   2h ago
WORK · phoenix-ide                    PR #375
```

For urgent/actionable states, state may lead more strongly:

```text
● Needs approval                      just now
conversation-slug                     WORK · phoenix-ide
```

Visible by default:

- status/actionability
- slug/title
- updated relative time
- mode badge/label
- compact project/folder/work-context label
- PR badge when `cached_pr` exists

Hidden/de-emphasized by default:

- created time
- full cwd
- model id
- message count
- full worktree path
- UUID-like path fragments
- PR title/head/base

### Chain collapsed/default card

Mobile chains should default to a compact summary that surfaces latest/current work:

```text
▸ chain-name                          2 parts · PR #375
Latest #2 · WORK · just now           phoenix-ide
```

Required behavior:

- Chain title/button explicitly opens `/chains/:rootId`.
- Caret expands/collapses chain history.
- Latest-summary/body opens the latest conversation.
- Collapsed summary surfaces latest/current member status, recency, mode, compact context, and PR badge if latest/current member has `cached_pr`.
- PR badge in mobile chain summary is visual only for this first redesign.

### Chain expanded card

Expanded mobile chain shape:

```text
▾ chain-name                          2 parts · PR #375

  #1 Completed                        22h ago
     EXPLORE · phoenix-ide

  ● #2 Latest · WORK                  just now
     phoenix-ide · PR #375
```

Required behavior:

- Latest/current member receives full row emphasis.
- Completed non-latest members are compact historical rows.
- Active selected historical member remains visible/full enough to orient the user.
- Non-latest members with `working`, `needs_action`, or `error` must not be hidden as inert history.
- Historical compact rows should not show redundant PR badges by default.

## Visual grammar

Introduce a small mobile-specific visual grammar rather than ad hoc badges:

| Token | Role | Weight |
| --- | --- | --- |
| Status dot/chip | Current state/actionability | High for error/needs_action/working |
| Latest chip/label | Chain currentness | Medium-high |
| Mode chip/label | Work/explore/direct context | Medium |
| PR badge | Current/latest PR relevance | Medium-high |
| Chain count | Scope/history count | Low/medium |
| Context metadata | Project/folder/work context | Low |
| Actions menu | Secondary operations | Low, 44px tap target |

Avoid making every badge look equally important. In particular, `needs_action`, `error`, and `working` should visually outrank mode and generic metadata.

## Interaction and action scope

- Row body opens the standalone conversation or latest member summary.
- Chain title/button opens the chain page.
- Caret expands/collapses the chain.
- Chain menu actions must be explicitly chain-scoped:
  - Rename chain
  - Archive chain
  - Delete chain
- Conversation row menu actions must remain conversation-scoped:
  - Rename conversation
  - Archive conversation, where valid
  - Delete conversation, where valid
- Preserve 44px mobile tap targets.
- Mobile PR badges are non-clickable in this first redesign; desktop PR badges remain clickable links.

## Implementation notes

Likely code areas:

- `ui/src/components/ConversationList.tsx`
- `ui/src/components/ConversationList.test.tsx`
- `ui/src/pages/ConversationListPage.tsx`
- `ui/src/index.css`
- existing chain helpers in `ui/src/utils/chains.ts`

Prefer an explicit mobile/list-density concept over overloading `sidebarMode`. The current sidebar-only compacting condition is conceptually useful but should not be tied to desktop sidebar layout.

The implementation should reuse existing data:

- `Conversation.presentation_mode` through `getConvDisplayState`
- `Conversation.cached_pr`
- `Conversation.conv_mode_label`
- `Conversation.project_name`, `cwd`, `worktree_path`, or existing derived project label behavior
- chain grouping from `computeChainRoots` / `groupConversationsForSidebar`
- `latestMemberId` from grouped chain item

## Acceptance criteria

### Mobile layout

- At mobile viewport widths, standalone rows use a clear two-line hierarchy instead of the current dense metadata stack.
- Full cwd, model, created time, and message count are not all shown by default in mobile rows.
- A compact project/folder/work-context label remains visible for disambiguation.
- The mobile header is compact and gives `+ New` primary weight while keeping Active/Archived and auth status available.

### Chain behavior

- Mobile chain groups render as compact chain cards with explicit chain title/button and latest-summary/open target.
- Completed non-latest chain members are minimized on mobile full-page lists, not only in sidebar mode.
- Latest/current chain member is visually emphasized.
- Active chain member is auto-revealed and remains visible.
- Non-latest `working`, `needs_action`, and `error` members are not visually buried as completed history.
- Collapsed chain summary shows latest/current status, recency, mode/context, and PR badge when applicable.

### PR behavior

- Desktop PR badges remain clickable links.
- Mobile PR badges are rendered as non-clickable visual badges in this first redesign.
- Standalone mobile rows show PR badge when `cached_pr` exists.
- Chain collapsed summary and latest member show PR badge when the latest/current member has `cached_pr`.
- Compact historical chain rows do not show redundant PR badges by default.

### Action safety

- Chain page navigation and latest-conversation navigation are distinct tap targets.
- Chain-scoped actions are labeled as chain-scoped.
- Conversation-scoped actions remain labeled as conversation-scoped.
- Mobile action hit targets are at least 44px where practical.

### Tests

Add or update tests covering:

- Mobile chain compact/default rendering.
- Mobile completed non-latest chain minimization in full-page mode.
- Active chain member auto-expansion/reveal behavior still works.
- Latest/current chain member receives PR badge; historical compact member does not.
- Standalone mobile PR badge is visual/non-link while desktop PR badge remains a link.
- Chain title/button navigates to `/chains/:rootId`; latest summary opens the latest conversation.
- `needs_action`, `working`, and `error` members are not minimized as inert completed history.

## Validation

Run the relevant UI tests and type checks through the project workflow. If practical, manually inspect a mobile viewport with seeded conversations containing:

- standalone active conversation with PR
- chain with completed historical member and latest work member with PR
- chain with a non-latest member needing action or error
- archived conversations
- long cwd/worktree paths

## Non-goals

- Do not redesign the desktop sidebar beyond preserving existing desktop PR-link behavior.
- Do not add new backend fields unless an existing data gap is discovered.
- Do not build a full row expansion/details system unless needed to satisfy the acceptance criteria.
- Do not make mobile PR badges external links in this first pass.
