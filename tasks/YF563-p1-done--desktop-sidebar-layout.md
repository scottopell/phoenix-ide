---
priority: p1
status: done
created: 2025-02-22
  - REQ-UI-016
  - REQ-UI-017
  - REQ-UI-018
artifact: completed
---

# Desktop Sidebar Layout

Implement persistent sidebar layout for desktop viewports (> 1024px). 

**Specs:** `specs/ui/requirements.md` (REQ-UI-016, 017, 018) and `specs/ui/design.md` (Responsive Layout Architecture, New Conversation Flows sections).

## Layout Overview

```
┌──────────────┬────────────────────────────────────────┐
│ [🔥 Phoenix] │  Main content (conversation or new)    │
│ [+ New]      │                                        │
│              │                                        │
│ ● conv-1     │                                        │
│   conv-2     │                                        │
│   conv-3     │                                        │
│              ├────────────────────────────────────────┤
│              │  Input / State Bar                     │
│ [◀ collapse] │                                        │
└──────────────┴────────────────────────────────────────┘
```

## Key Components

| Component | File | Purpose |
|-----------|------|--------|
| `DesktopLayout` | `ui/src/components/DesktopLayout.tsx` | Responsive wrapper, renders sidebar on desktop |
| `Sidebar` | `ui/src/components/Sidebar.tsx` | Phoenix icon, "+ New", ConversationList, collapse toggle |
| `SidebarNewForm` | `ui/src/components/SidebarNewForm.tsx` | Compact inline new-conversation form (REQ-UI-018) |

## Route Behavior Summary

| Route | Main Content | Sidebar "+ New" Behavior |
|-------|--------------|---------------------------|
| `/` | NewConversationPage (full form) | No-op (already on new conversation view) |
| `/c/:slug` | ConversationPage | Expand inline form at top of sidebar |

## New Conversation Forms

Both the full-page form (REQ-UI-017) and inline sidebar form (REQ-UI-018) must provide:

1. **Directory picker** - reuse `DirectoryPicker` component
2. **Model selector** - reuse from `SettingsFields`
3. **Message input** - textarea for initial message
4. **Send button** - creates conversation and navigates to it
5. **Send in Background button** - creates conversation, does NOT navigate, shows toast confirmation

### Send in Background Flow

```typescript
async function sendInBackground(data: NewConvData) {
  const conv = await api.createConversation(data);
  await api.sendMessage(conv.id, data.message);
  showToast(`Started: ${conv.slug}`);
  // Do NOT navigate - user stays where they are
  // For inline form: collapse form
  // For full page: stay on `/` for another new conversation
}
```

## Files to Modify

- `ui/src/App.tsx` - wrap router outlet with `DesktopLayout`
- `ui/src/components/DesktopLayout.tsx` - new
- `ui/src/components/Sidebar.tsx` - new
- `ui/src/components/SidebarNewForm.tsx` - new
- `ui/src/components/ConversationList.tsx` - add `compact` prop for collapsed view
- `ui/src/pages/NewConversationPage.tsx` - add "Send in Background" button
- `ui/src/index.css` - sidebar styles

## Sidebar Details

### Header Section
- **Phoenix icon** (favicon/logo) - clicking navigates to `/`
- **"+ New" button** - behavior depends on current route (see table above)

### Conversation List
- Reuse existing `ConversationList` component
- State indicators already implemented (task 561)
- Active conversation highlighted (match by slug from URL)

### Collapsed State
- Collapsed width: ~48px
- Show only state indicator dots for recent conversations
- Hover expands temporarily, click to toggle permanently
- Persist preference: `localStorage.setItem('sidebar-collapsed', 'true')`

### Inline New Form (REQ-UI-018)
- Expands at top of sidebar when "+ New" clicked from `/c/:slug`
- Compact layout (stacked fields)
- Cancel button + Escape key to dismiss
- Submit collapses form and navigates (or stays if "Send in Background")

## Out of Scope

- Tablet-specific behavior (768-1024px) - uses mobile layout
- Drag to resize sidebar
- Mobile bottom sheet updates (already done in task 561)

## Acceptance Criteria

**REQ-UI-016 (Sidebar Layout):**
- [x] Sidebar visible on viewports > 1024px
- [x] Phoenix icon at top, clicking navigates to `/`
- [x] "+ New" button below icon
- [x] Conversation list with state indicators (green/yellow/red dots)
- [x] Active conversation highlighted
- [x] Clicking conversation switches main content without full-page reload
- [x] Sidebar collapsible to icon strip via toggle button
- [x] Collapsed state shows state dots for recent conversations
- [x] Collapse preference persisted in localStorage

**REQ-UI-017 (Full Page Mode):**
- [x] Route `/` renders NewConversationPage in main content area
- [x] Sidebar visible with no active conversation highlighted
- [x] "+ New" click is no-op when on `/`
- [x] Form has Send and Send in Background buttons
- [x] Send creates conversation and navigates to it
- [x] Send in Background creates conversation, stays on `/`, shows toast

**REQ-UI-018 (Inline Sidebar Mode):**
- [x] "+ New" from `/c/:slug` expands inline form at top of sidebar
- [x] Current conversation remains visible in main content
- [x] Form has directory, model, message fields
- [x] Form has Send and Send in Background buttons
- [x] Send creates, navigates, collapses form
- [x] Send in Background creates, stays on current conversation, collapses form, shows toast
- [x] Cancel button and Escape key dismiss form without action

**General:**
- [x] Below 1024px: no sidebar, current mobile/tablet behavior unchanged
- [x] No regressions to existing conversation or new conversation flows
