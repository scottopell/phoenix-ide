# Phoenix IDE — Adaptive Desktop/Mobile UX Audit

**Date:** 2025-02-22  
**Auditor context:** Single power user, genuine dual form-factor usage (desktop + mobile)  
**Codebase version:** `38fc610c` | 230 active conversations | React + react-router SPA

---

## 1. Current State Summary

### Information Architecture

Phoenix uses a **three-page SPA** architecture with full-page route transitions:

```
/            → ConversationListPage  (full-screen list)
/new         → NewConversationPage   (full-screen form)
/c/:slug     → ConversationPage      (full-screen chat)
```

Each page is a complete viewport takeover. There is **no persistent navigation chrome** — no sidebar, no drawer, no tab bar. Navigation between pages is through:

- Clickable slug link (← arrow + name) in the StateBar at bottom of ConversationPage
- "← Back" link at top-left of NewConversationPage  
- `Escape` keyboard shortcut (returns to `/` from any sub-page)
- `n` key on list page creates new conversation
- `j`/`k` or arrow keys navigate the list

### Layout Strategy

The layout is fundamentally **mobile-first single-column**. The only responsive breakpoint is `768px`, which toggles `.mobile-only` / `.desktop-only` visibility classes. This affects only the NewConversationPage (settings card position and input placement). The conversation list and conversation view are identical across all viewport sizes.

The ConversationPage layout stacks vertically:
```
┌─────────────────────────┐
│ Messages (scrollable)   │  flex: 1 1 auto
├─────────────────────────┤
│ Input Area              │  flex: 0 0 auto  
├─────────────────────────┤
│ Breadcrumb Bar          │  36px
├─────────────────────────┤
│ State Bar (← slug, ●)   │  52px
└─────────────────────────┘
```

On a 1440px-wide desktop, messages stretch the full viewport width. On a 390px phone, the same. There is no max-width constraint on messages — content spans edge-to-edge.

### What Works Well

- **The mobile conversation experience is solid.** Full-height messages with bottom-anchored input is the correct mobile pattern. The 100dvh handling, safe-area-inset-bottom padding, and iOS keyboard awareness (useIOSKeyboardFix) show genuine mobile-first thinking.
- **State bar is information-dense and useful.** Slug, model, cwd, connection status, context window meter — all visible at a glance without separate screens.
- **Breadcrumb trail during agent execution** provides live progress awareness, especially valuable when the agent is running tools you can't see yet.
- **Keyboard navigation on the list page** (j/k/Enter/n/Escape) is vim-fluent and well-implemented.
- **Pull-to-refresh on conversation list** is a thoughtful mobile gesture.
- **Offline/reconnection handling** with queued messages, optimistic UI, and SSE reconnection is robust.
- **Draft persistence** (useDraft, localStorage) means you never lose a half-typed message.
- **Scroll position save/restore** on the conversation list is correctly implemented via sessionStorage.

---

## 2. Breakpoint Analysis

### Desktop (> 1024px)

The desktop experience is a mobile app rendered in a very large viewport.

| Aspect | Assessment |
|--------|------------|
| Conversation list | Full-width cards with enormous horizontal dead space. 230 items rendered as a flat scroll list (~29,000px). No virtualization. |
| Chat messages | Text spans full 1440px — line lengths exceed 200 characters, well past the 60-80 character readability optimum. |
| New conversation | Centered input box is the correct desktop pattern. Settings collapsible row works well. |
| Space utilization | ~60-70% of desktop horizontal real estate is dead (empty padding on either side of content). No secondary panel, no sidebar, no supplementary information. |
| Navigation | Escape → list → click → conversation requires 3 interactions minimum to switch conversations. |

### Tablet (768px – 1024px)

Tablet inherits the desktop breakpoint (> 768px), so it shows the desktop-only elements. This is actually the **worst-served form factor**:

| Aspect | Assessment |
|--------|------------|
| NewConversationPage | Shows desktop layout (centered input + collapsible settings row) rather than mobile layout. At 820px this works but barely — the centered input box is comfortably wide enough. |
| Conversation list | Same as desktop — full-width cards, no space economy issues at this width. |
| Chat | Line lengths are still too long but more tolerable (~100-120 chars). |
| Overall | Tablet gets desktop chrome without desktop space. No unique tablet optimization. |

### Mobile (< 768px)

| Aspect | Assessment |
|--------|------------|
| Conversation list | Well-adapted. Cards fill width naturally. Touch targets adequate. |
| Chat | Good. Messages naturally constrain to readable widths. Bottom-anchored input correct. |
| New conversation | Settings card at top, input at bottom — correct mobile pattern. |
| Navigation | Full page transitions are acceptable on mobile (expected mental model). |
| Breadcrumb bar | Horizontal scroll works but can be hard to parse with many items. |

---

## 3. Problem Severity Matrix

| # | Problem | Impact | Frequency | Desktop | Tablet | Mobile | Severity |
|---|---------|--------|-----------|---------|--------|--------|----------|
| 1 | **Full-page navigation between list and chat** — no way to see other conversations while in a conversation | High | Every session, multiple times | ★★★ | ★★★ | ★☆☆ | **Critical** |
| 2 | **No conversation search/filter** — 230 conversations, flat chronological list, no search | High | Growing with usage | ★★★ | ★★★ | ★★★ | **Critical** |
| 3 | **Message content width unbounded on desktop** — text spans full 1440px+, unreadable line lengths | Medium | Constant on desktop | ★★★ | ★★☆ | ☆☆☆ | **High** |
| 4 | **New conversation is a separate page** — loses all conversation context, full screen for 2 fields + textarea | Medium | Several times/day | ★★★ | ★★★ | ★☆☆ | **High** |
| 5 | **No conversation list virtualization** — 230 items rendered, ~29K px scroll height, will degrade with scale | Low now | Growing | ★★☆ | ★★☆ | ★★☆ | **Medium** |
| 6 | **Context switching friction** — minimum 3 interactions to jump between two conversations | Medium | Multiple times/session | ★★★ | ★★★ | ★★☆ | **High** |
| 7 | **Breadcrumb bar unreadable at scale** — 20+ breadcrumbs become a wall of indistinguishable chips | Low | Long conversations | ★★☆ | ★★☆ | ★★★ | **Medium** |
| 8 | **No scroll position memory within conversations** — leaving and returning always shows last position, but returning to a conversation you scrolled up in resets to bottom | Low | Occasional | ★★☆ | ★★☆ | ★★☆ | **Low** |
| 9 | **State bar at bottom, semantic header** — `<header>` element rendered at screen bottom is an accessibility/semantic mismatch | Low | Constant | ★☆☆ | ★☆☆ | ★☆☆ | **Low** |
| 10 | **No keyboard shortcut to jump to specific conversation** — can't Cmd+K or fuzzy-find | Medium | Power user need | ★★★ | ☆☆☆ | ☆☆☆ | **Medium** |

---

## 4. Principle Violations

### 4.1 Progressive Disclosure → Violated by Full-Page Navigation

**Principle:** Information should be available without removing it from view entirely.

**Violation:** Entering any conversation completely removes awareness of all other conversations. The user must fully exit the conversation (navigate to `/`) to see the list, then navigate back. This is a **binary toggle between two states** rather than progressive disclosure.

On desktop, where there's 1440px of width, there is zero reason the conversation list can't remain visible. On mobile, full-page transitions are acceptable but should support gesture-based reveal (swipe from edge).

### 4.2 Consistent Information Scent → Violated by New Conversation Page

**Principle:** Users should know what actions are available from any state.

The `/new` page is a dead end. The only escape is "← Back" to the list. There's no way to:
- See your recent conversations for context
- Quick-switch to an existing conversation you meant to continue
- Preview what directory/model your recent conversations used

### 4.3 Adaptive, Not Responsive Alone → Single Breakpoint is Insufficient

**Principle:** Layout should adapt intelligently, not just shrink.

There's only one breakpoint at 768px, and it only affects the new-conversation page. The conversation view and list are pixel-identical at 390px and 2560px. Desktop gets a mobile layout stretched wide. Mobile gets a layout that happens to fit. Neither is optimized for its context.

### 4.4 State Awareness → Scroll Position Not Preserved Across Conversations

**Principle:** The app should remember scroll position, active conversation, recent filters.

Scroll position is saved for the **conversation list** (sessionStorage), which is good. But there's no scroll position memory **per conversation**. If you're reading a long conversation mid-scroll, switch to another, and return — you're at the bottom, not where you were.

### 4.5 Efficiency → Conversation Switching is a Multi-Step Journey

**Principle:** Common workflows should require minimal interaction.

The "check another conversation then come back" workflow:
1. Click ← slug link (or press Escape)
2. Wait for list to load/render (230 items)
3. Scroll to find the other conversation (or use j/k)
4. Click it
5. Wait for conversation + SSE to load
6. Read what you needed
7. Click ← again
8. Find original conversation
9. Click it
10. Wait for load again

That's **10 interactions** for what should be 2 (click other thread, click back). And scroll position in the original conversation is lost.

---

## 5. Critical Pain Points

### Pain Point 1: "Where Was I?"
**When:** Returning to a conversation after viewing the list or another conversation.  
**What happens:** The conversation reloads from scratch (cache then network). You're placed at the bottom. If you were reading something mid-conversation, that position is gone.  
**Impact:** Cognitive load — user must re-orient. On long conversations (104 messages in `browser-console-log-retrieval`), re-finding your place is painful.

### Pain Point 2: "Which One Was It?"
**When:** Looking for a specific past conversation in a 230-item flat list.  
**What happens:** Scroll. And scroll. Conversation slugs are auto-generated and often cryptic (`tuesday-morning-thunder-frost`). No search, no filter, no grouping by date/directory/model.  
**Impact:** Grows linearly worse with usage. At 500 conversations this becomes untenable.

### Pain Point 3: "I Just Need to Start a Quick Thread"
**When:** In the middle of monitoring one conversation, want to kick off another.  
**What happens:** Navigate to `/new`, which is a full-screen takeover centered around a text input. All context about what's running is gone. After creating, you're in the new conversation — no way to quickly peek at the one you were monitoring.  
**Impact:** Discourages parallel workflows. User delays starting new conversations to avoid losing their place.

### Pain Point 4: "Desktop Is a Phone App in a Big Window"
**When:** Any desktop session.  
**What happens:** 1440px of width, 60% is empty. Messages are too wide to read comfortably. No secondary content panels. Context that could be shown simultaneously requires page navigation.  
**Impact:** Constant, low-grade inefficiency. Desktop users get no benefit from their extra space.

### Pain Point 5: "What's Running Right Now?"
**When:** Multiple conversations might be active (agent working in one, idle in another).  
**What happens:** You can only see the state of ONE conversation — the one you're viewing. The list page shows conversation metadata but NOT current state (idle/working/error).  
**Impact:** Missed awareness. You have to check each conversation individually to know if an agent finished or errored.

---

## 6. Recommended Adaptive Pattern

### Core Principle: **List/Detail with Adaptive Presentation**

The fundamental pattern should be **list/detail** (also called master/detail), where the conversation list and the active conversation are logically paired, but rendered differently based on available space.

### Desktop (> 1024px): **Persistent Sidebar + Main Content**

```
┌──────────────┬────────────────────────────────────────┐
│ Conv List    │  Messages                              │
│ [search]     │  (max-width constrained, centered)     │
│              │                                        │
│ • conv-1  ● │                                        │
│   conv-2    │                                        │
│   conv-3    │                                        │
│              │                                        │
│              ├────────────────────────────────────────┤
│              │  Input Area                            │
│              ├────────────────────────────────────────┤
│              │  Breadcrumbs │ State Bar               │
│ [+ New]      │                                        │
└──────────────┴────────────────────────────────────────┘
```

**Key details:**
- Sidebar width: ~280-320px, collapsible to icon-width (~48px) via toggle or keyboard shortcut
- Conversation list items show **live state indicators** (green dot = idle, yellow pulse = working, red = error)
- Search/filter input at top of sidebar — instant filter by slug, model, directory
- Message area capped at `max-width: 800px` and centered within remaining space, creating comfortable line lengths
- "+ New" in sidebar opens an **inline new-conversation panel** at the top of the sidebar or a compact modal — NOT a separate page
- Active conversation visually distinct in sidebar (background highlight + left border accent)
- Sidebar scroll position independent of main content scroll

**New conversation on desktop:** The new conversation form could either:
- (a) Replace the main content area while keeping the sidebar visible, or
- (b) Be a compact inline expansion at the top of the sidebar (just directory + model + textarea), or  
- (c) Be an overlay/modal that doesn't destroy the current view

Option (b) is the most efficient — you can kick off a new conversation without leaving the current one.

### Mobile (< 768px): **Full-Page with Gesture Navigation**

The current mobile pattern is mostly correct. Refine it:

```
List Page                    Chat Page
┌──────────────┐            ┌──────────────┐
│ [search]     │            │ Messages     │
│              │ ──tap──▶   │              │
│ • conv-1  ● │            │              │
│   conv-2    │ ◀──swipe── │              │
│   conv-3    │            ├──────────────┤
│              │            │ Input        │
│              │            ├──────────────┤
│ [+ New]      │            │ State Bar    │
└──────────────┘            └──────────────┘
```

**Key changes:**
- **Swipe-from-left-edge** reveals conversation list as a full-height overlay (like iOS back gesture but showing the list)
- Conversation list items show **live state dots** so you can see at a glance what's working
- **Search bar** at top of list — critical at 230+ conversations
- "+ New" becomes a FAB (floating action button) or stays as header button but opens a **bottom sheet** rather than a full page — shows directory, model, textarea in a half-screen sheet that can be swiped to dismiss
- Optional: long-press a conversation in the list shows a quick-peek preview (last message) before committing to navigation

### Tablet (768px – 1024px): **Collapsible Sidebar**

Tablet gets the sidebar layout but with the sidebar **collapsed by default** to a narrow icon strip showing just state dots for recent conversations. Tap to expand.

```
┌────┬──────────────────────────────────┐
│ ●  │  Messages                        │
│ ●  │  (comfortable width at 700px)    │
│ ○  │                                  │
│    │                                  │
│    ├──────────────────────────────────┤
│    │  Input Area                      │
│ +  │  State Bar                       │
└────┴──────────────────────────────────┘
```

Tap the collapsed sidebar → slides out to full 280px panel, overlaying content (not pushing it). This gives tablet users quick access without sacrificing content width.

### Quick-Switcher (All Form Factors)

Add a **Cmd+K / Ctrl+K** fuzzy-find overlay:
- Shows recent conversations with live state
- Type to filter by slug, directory, or model
- Arrow keys + Enter to select
- On mobile: accessed via a small button in the state bar or conversation list header

This single feature addresses problems #2, #6, and #10 from the severity matrix simultaneously.

---

## 7. Implementation Priority

Ordered by **impact × effort efficiency** — what delivers the most improvement for the least disruption to existing code:

### Phase 1: Quick Wins (High Impact, Low Risk)

1. **Constrain message width on desktop** — Add `max-width: 800px` and `margin: 0 auto` to the message container inside `#main-area`. Pure CSS, zero component changes. Immediately fixes readability.

2. **Add search/filter to conversation list** — Input field at top of ConversationList that filters the displayed list by slug substring match. Small component change, huge discovery improvement at 230+ conversations.

3. **Show live state indicators on conversation list** — Add a small dot to each conversation card showing its current state. Requires a lightweight polling or SSE multiplexing mechanism, but eliminates the "what's running?" blind spot.

### Phase 2: Layout Foundation (High Impact, Moderate Effort)

4. **Introduce sidebar layout on desktop** — Wrap the router in a responsive layout component that shows the conversation list as a persistent sidebar above the 1024px breakpoint. The ConversationList component already exists and is well-factored — it just needs to be rendered alongside the conversation view instead of on a separate route.

5. **Inline new-conversation in sidebar** — When sidebar is visible, "+ New" expands a compact form inline rather than navigating to `/new`. The settings fields are already extracted as a reusable `SettingsFields` component — reuse it.

6. **Quick-switcher overlay (Cmd+K)** — Fuzzy-find modal that works across all form factors. Independent of the sidebar work.

### Phase 3: Mobile Polish (Medium Impact, Moderate Effort)

7. **Bottom sheet for new conversation on mobile** — Replace full-page `/new` with a bottom sheet that slides up from the input area.

8. **Swipe gesture for back-navigation on mobile** — Swipe from left edge to reveal conversation list overlay.

9. **Per-conversation scroll position memory** — Store scroll position per conversation ID before navigating away, restore on return.

### Phase 4: Scale Preparation (Future-Proofing)

10. **Conversation list virtualization** — Replace flat rendering with a virtual list (react-window or similar). 230 items is fine today, 1000+ will not be.

11. **Conversation grouping/sections** — Group by date (Today, Yesterday, This Week, Older) or allow grouping by directory/model.

12. **Multi-conversation state awareness** — Lightweight SSE or polling to show active state across all conversations in the sidebar/list.

---

## Appendix: Audit Evidence

### Screenshots Captured

| Viewport | Page | Observation |
|----------|------|-------------|
| 1440×900 | List | Full-width cards, enormous dead space left/right |
| 1440×900 | Conversation | Messages span full width, unreadable line lengths |
| 1440×900 | New conversation | Centered input is correct; settings collapsible row works |
| 1440×900 | Long conversation (104 msgs) | Breadcrumb bar with 20+ items is a solid wall of chips |
| 820×1180 | List | Desktop layout at tablet size — fine but unoptimized |
| 820×1180 | Conversation | Works, line lengths tolerable |
| 390×844 | List | Good mobile list rendering |
| 390×844 | Conversation | Solid mobile chat experience |
| 390×844 | New conversation | Settings at top, input at bottom — correct pattern |

### Component Architecture Notes

- `ConversationList` is **already decoupled** from `ConversationListPage` — it receives props and emits callbacks. This is excellent for Phase 2 (sidebar reuse).
- `SettingsFields` in NewConversationPage is **already extracted** as a reusable internal component. Ready for inline sidebar form.
- `useKeyboardNav` hook is well-structured and composable. Extending to quick-switcher should be straightforward.
- The `appMachine` XState machine handles offline/online transitions — sidebar state could be added as an orthogonal state.
- No existing test coverage for layout behavior, but the state machine has property tests. Layout changes are pure presentation — lower test risk.

### Keyboard Shortcuts Inventory

| Key | Context | Action |
|-----|---------|--------|
| `Escape` | Any input | Blur |
| `Escape` | Conversation/New page | Navigate to list |
| `j` / `↓` | List page | Select next conversation |
| `k` / `↑` | List page | Select previous conversation |
| `Enter` | List page (with selection) | Open selected conversation |
| `n` | List page | New conversation |
| `/` | Conversation page | Focus message input |
| `g` | List page | Go to top |

Missing: `Cmd+K` (quick switcher), `[` / `]` (prev/next conversation), `Cmd+N` (new conversation from anywhere).
