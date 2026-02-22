# Web UI Design

This document describes the technical architecture implementing requirements in `specs/ui/requirements.md`.

## Technology Stack

- **React 18** with TypeScript
- **React Router v6** for client-side routing
- **XState-style state machines** for complex state (app, connection)
- **Vite** for development and building
- **CSS Variables** for theming (no CSS framework)
- **localStorage** for persistence

## Architecture Overview

```
ui/src/
├── main.tsx                    # Entry point
├── App.tsx                     # Router setup, layout wrapper
├── api.ts                      # API client and types
├── cache.ts                    # Offline cache management
├── syncQueue.ts                # Background sync queue
├── types.ts                    # Shared types
├── utils.ts                    # Shared utilities
│
├── machines/
│   └── appMachine.ts           # App-level state (network, sync)
│
├── hooks/
│   ├── useAppMachine.ts        # App state machine hook
│   ├── useConnection.ts        # SSE connection management
│   ├── connectionMachine.ts    # Connection state machine
│   ├── useDraft.ts             # Draft message persistence
│   ├── useMessageQueue.ts      # Offline message queue
│   ├── useKeyboardNav.ts       # List keyboard navigation
│   ├── useLocalStorage.ts      # localStorage wrapper
│   ├── useIOSKeyboardFix.ts    # iOS viewport fix
│   ├── useTheme.ts             # Dark/light theme
│   └── useToast.tsx            # Toast notifications
│
├── pages/
│   ├── ConversationListPage.tsx  # Mobile: full-page list
│   ├── ConversationPage.tsx      # Chat view
│   └── NewConversationPage.tsx   # New conversation form
│
├── components/
│   ├── ConversationList.tsx      # Reusable list (pages + sidebar)
│   ├── MessageList.tsx           # Message display
│   ├── MessageComponents.tsx     # Message rendering (markdown, tools)
│   ├── InputArea.tsx             # Message composition
│   ├── StateBar.tsx              # Bottom bar (slug, model, status)
│   ├── BreadcrumbBar.tsx         # Agent activity trail
│   ├── SettingsFields.tsx        # Directory/model pickers (reusable)
│   ├── DirectoryPicker.tsx       # Directory selection with validation
│   ├── NewConversationSheet.tsx  # Mobile bottom sheet
│   ├── ImageAttachments.tsx      # Image upload/preview
│   ├── VoiceInput/               # Voice recording components
│   ├── Toast.tsx                 # Toast notifications
│   ├── ConfirmDialog.tsx         # Confirmation modal
│   ├── RenameDialog.tsx          # Conversation rename
│   └── ...                       # Other UI components
│
└── utils/
    ├── images.ts                 # Image processing
    ├── linkify.tsx               # URL detection
    └── uuid.ts                   # UUID generation
```

## Responsive Layout Architecture (REQ-UI-010, REQ-UI-016)

The app uses viewport-based layout switching:

```
┌─────────────────────────────────────────────────────────────┐
│  Mobile (< 768px)        │  Desktop (> 1024px)             │
│                          │                                 │
│  ┌────────────────────┐   │  ┌──────────┬──────────────────┐  │
│  │                    │   │  │ Sidebar  │ Main Content     │  │
│  │   Full-page view   │   │  │          │                  │  │
│  │   (list OR chat)   │   │  │ [+ New]  │  Conversation    │  │
│  │                    │   │  │ conv-1   │  or NewConv form │  │
│  │                    │   │  │ conv-2   │                  │  │
│  │                    │   │  │ conv-3   │                  │  │
│  └────────────────────┘   │  └──────────┴──────────────────┘  │
│                          │                                 │
└──────────────────────────┴─────────────────────────────────┘
```

### Desktop Layout (REQ-UI-016)

```typescript
function DesktopLayout({ children }: { children: React.ReactNode }) {
  const isDesktop = useMediaQuery('(min-width: 1024px)');
  const [isCollapsed, setCollapsed] = useLocalStorage('sidebar-collapsed', false);
  
  if (!isDesktop) return <>{children}</>;
  
  return (
    <div className="desktop-layout">
      <Sidebar collapsed={isCollapsed} onToggle={() => setCollapsed(!isCollapsed)} />
      <main className="desktop-main">{children}</main>
    </div>
  );
}
```

### Sidebar Component

```typescript
interface SidebarProps {
  collapsed: boolean;
  onToggle: () => void;
}

function Sidebar({ collapsed, onToggle }: SidebarProps) {
  const location = useLocation();
  const [newFormOpen, setNewFormOpen] = useState(false);
  
  const handleNewClick = () => {
    if (location.pathname === '/') return; // No-op on root
    setNewFormOpen(true); // Expand inline form
  };
  
  return (
    <aside className={`sidebar ${collapsed ? 'collapsed' : ''}`}>
      <PhoenixIcon onClick={() => navigate('/')} />
      <button onClick={handleNewClick}>+ New</button>
      {newFormOpen && <SidebarNewForm onClose={() => setNewFormOpen(false)} />}
      <ConversationList compact={collapsed} />
      <button onClick={onToggle}>{collapsed ? '▶' : '◀'}</button>
    </aside>
  );
}
```

## New Conversation Flows (REQ-UI-015, REQ-UI-017, REQ-UI-018)

Three modes for creating conversations, all functionally equivalent:

| Mode | Trigger | Location | Send Options |
|------|---------|----------|-------------|
| Mobile Bottom Sheet | "+ New" on mobile | Overlay on current view | Send, Send in Background |
| Desktop Full Page | Navigate to `/` | Main content area | Send, Send in Background |
| Desktop Inline Sidebar | "+ New" from `/c/:slug` | Top of sidebar | Send, Send in Background |

### Shared Form Component

```typescript
interface NewConversationFormProps {
  layout: 'full' | 'sheet' | 'inline';
  onSubmit: (data: NewConvData, background: boolean) => void;
  onCancel?: () => void;
}

function NewConversationForm({ layout, onSubmit, onCancel }: NewConversationFormProps) {
  return (
    <form className={`new-conv-form new-conv-form--${layout}`}>
      <DirectoryPicker ... />
      <ModelSelector ... />
      <MessageInput ... />
      <div className="actions">
        {onCancel && <button type="button" onClick={onCancel}>Cancel</button>}
        <button type="button" onClick={() => onSubmit(data, true)}>Send in Background</button>
        <button type="submit">Send</button>
      </div>
    </form>
  );
}
```

### Send in Background Flow

```typescript
async function sendInBackground(data: NewConvData) {
  const conv = await api.createConversation(data);
  await api.sendMessage(conv.id, data.message);
  showToast(`Started: ${conv.slug}`);
  // Do NOT navigate - user stays where they are
}
```

## Conversation State Indicators (REQ-UI-012)

Each conversation displays its current state:

```typescript
type ConvDisplayState = 'idle' | 'working' | 'error';

function getDisplayState(state: ConvState): ConvDisplayState {
  if (state === 'Idle') return 'idle';
  if (state.type === 'Error') return 'error';
  return 'working'; // All other states
}

function StateIndicator({ state }: { state: ConvState }) {
  const display = getDisplayState(state);
  return (
    <span className={`state-dot state-dot--${display}`} />
  );
}
```

### Visual States

| State | Color | Animation |
|-------|-------|----------|
| idle | Green | None |
| working | Yellow | Pulse |
| error | Red | None |

### Polling for Updates

Conversation list polls for state changes:

```typescript
function useConversationListPolling(intervalMs = 5000) {
  const [conversations, setConversations] = useState<Conversation[]>([]);
  
  useEffect(() => {
    const poll = async () => {
      const data = await api.getConversations();
      setConversations(data);
    };
    
    poll();
    const interval = setInterval(poll, intervalMs);
    return () => clearInterval(interval);
  }, [intervalMs]);
  
  return conversations;
}
```

## Scroll Position Memory (REQ-UI-013)

Per-conversation scroll position preserved across navigation:

```typescript
const SCROLL_KEY = (id: string) => `phoenix:scroll:${id}`;

function useScrollMemory(conversationId: string, containerRef: RefObject<HTMLElement>) {
  // Restore on mount
  useEffect(() => {
    const saved = localStorage.getItem(SCROLL_KEY(conversationId));
    if (saved && containerRef.current) {
      containerRef.current.scrollTop = parseInt(saved, 10);
    }
  }, [conversationId]);
  
  // Save on unmount or navigation
  useEffect(() => {
    return () => {
      if (containerRef.current) {
        localStorage.setItem(
          SCROLL_KEY(conversationId),
          String(containerRef.current.scrollTop)
        );
      }
    };
  }, [conversationId]);
}
```

### New Messages While Away

When returning to a conversation with new messages:

```typescript
function MessageList({ conversationId, messages }) {
  const [savedPosition, setSavedPosition] = useState<number | null>(null);
  const [hasNewMessages, setHasNewMessages] = useState(false);
  
  // Detect new messages since last visit
  useEffect(() => {
    const lastSeen = localStorage.getItem(`phoenix:lastSeen:${conversationId}`);
    const newestMsg = messages[messages.length - 1];
    if (lastSeen && newestMsg && newestMsg.sequence_id > parseInt(lastSeen)) {
      setHasNewMessages(true);
    }
  }, [messages]);
  
  return (
    <div ref={containerRef}>
      {messages.map(m => <Message key={m.id} ... />)}
      {hasNewMessages && savedPosition !== null && (
        <button className="jump-to-new" onClick={scrollToBottom}>
          ↓ Jump to newest
        </button>
      )}
    </div>
  );
}
```

## Message Delivery State Machine (REQ-UI-004)

```
[draft] --send--> [sending] --success--> [sent] ✓
                      |
                      +--error--> [failed] --retry--> [sending]
```

### States

| State | Visual | Stored In | Behavior |
|-------|--------|-----------|----------|
| draft | (none) | localStorage | Persisted per conversation, restored on load |
| sending | ⏳ spinner | localStorage (queue) | Waiting for server confirmation |
| sent | ✓ checkmark | server | Confirmed by API |
| failed | ⚠️ tap to retry | localStorage (queue) | Retryable, persists across refresh |

### localStorage Schema (REQ-UI-011)

```typescript
// Draft message (one per conversation)
localStorage.setItem(`phoenix:draft:${convId}`, "partial message text");

// Message queue (unsent messages - sending or failed)
localStorage.setItem(`phoenix:queue:${convId}`, JSON.stringify([
  { localId: "uuid", text: "message 1", status: "sending", timestamp: 1234567890 },
  { localId: "uuid", text: "message 2", status: "failed", timestamp: 1234567891 }
]));

// Last sequence ID (for reconnection)
localStorage.setItem(`phoenix:lastSeq:${convId}`, "42");

// Scroll position (per conversation)
localStorage.setItem(`phoenix:scroll:${convId}`, "1234");

// Sidebar collapsed preference
localStorage.setItem(`sidebar-collapsed`, "true");
```

## Connection State Machine (REQ-UI-005, REQ-UI-006)

```
[disconnected] --connect--> [connecting] --open--> [connected]
                                 |                      |
                                 +--error--> [reconnecting]
                                                  |
                                 +--------<-------+
                                 |                |
                            (backoff)        (success)
                                 |                |
                                 v                v
                           [reconnecting]   [connected]
                                 |
                           (3+ failures)
                                 |
                                 v
                            [offline]
                                 |
                           (keep trying)
```

### Reconnection with Sequence Tracking

```typescript
function reconnect() {
  const after = lastSequenceId.current 
    || localStorage.getItem(`phoenix:lastSeq:${convId}`);
  const url = after 
    ? `/api/conversations/${convId}/stream?after=${after}`
    : `/api/conversations/${convId}/stream`;
  eventSource = new EventSource(url);
}

// Dedupe safety net
const seenIds = new Set<number>();
function handleMessage(msg: Message) {
  if (seenIds.has(msg.sequence_id)) return;
  seenIds.add(msg.sequence_id);
  // ... process message
}
```

### Backoff Strategy

```typescript
const BACKOFF_BASE = 1000;      // 1 second
const BACKOFF_MAX = 30000;      // 30 seconds
const OFFLINE_THRESHOLD = 3;    // Show "offline" after N failures

function getBackoffDelay(attempt: number): number {
  return Math.min(BACKOFF_BASE * Math.pow(2, attempt - 1), BACKOFF_MAX);
}
```

## App State Machine

Top-level state management for network and sync:

```typescript
type AppState =
  | { type: 'initializing' }
  | { type: 'ready'; network: NetworkState; sync: SyncStatus }
  | { type: 'error'; message: string };

type NetworkState = 'online' | 'offline' | 'unknown';

type SyncStatus = 
  | { type: 'idle' }
  | { type: 'syncing'; progress: number; total: number }
  | { type: 'error'; message: string; retryIn: number };
```

The app machine coordinates:
- Cache initialization
- Network state tracking (`navigator.onLine`)
- Background sync of queued operations
- User notifications (toasts)

## Visual States Reference

### Connection Indicator

| State | Dot | Text | Banner |
|-------|-----|------|--------|
| connected | 🟢 | "ready" | none |
| connecting | ⚪ | "connecting..." | none |
| reconnecting | 🟡 | "reconnecting (3)..." | none |
| offline | 🔴 | "offline" | "Reconnecting in 8s..." |

### Conversation State Indicators

| State | Dot | Meaning |
|-------|-----|--------|
| idle | 🟢 | Ready for input |
| working | 🟡 (pulse) | Agent processing |
| error | 🔴 | Error occurred |

### Message States

| State | Icon | Interactive |
|-------|------|-------------|
| draft | (none) | editable in input |
| sending | ⏳ | non-interactive |
| sent | ✓ | normal message |
| failed | ⚠️ | tap to retry |

## Desktop Message Readability (REQ-UI-014)

```css
.message-content {
  max-width: 800px;
  margin: 0 auto;
}

.message-content pre {
  overflow-x: auto;
  max-width: 100%;
}
```

## Component Responsibilities Summary

| Component | Responsibility | Requirements |
|-----------|---------------|-------------|
| `DesktopLayout` | Viewport detection, sidebar wrapper | REQ-UI-010, REQ-UI-016 |
| `Sidebar` | Conversation list, new form, collapse | REQ-UI-016, REQ-UI-018 |
| `ConversationList` | List display, state indicators, selection | REQ-UI-001, REQ-UI-012 |
| `NewConversationSheet` | Mobile bottom sheet | REQ-UI-015 |
| `NewConversationPage` | Full-page form | REQ-UI-017 |
| `SidebarNewForm` | Inline sidebar form | REQ-UI-018 |
| `MessageList` | Message display, scroll memory | REQ-UI-002, REQ-UI-013 |
| `InputArea` | Composition, drafts, queue | REQ-UI-003, REQ-UI-004 |
| `StateBar` | Connection status, context info | REQ-UI-005, REQ-UI-007 |
| `BreadcrumbBar` | Agent activity trail | REQ-UI-007 |
