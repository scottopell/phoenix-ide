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

## Typed Conversation State (REQ-UI-007, REQ-UI-020)

### Discriminated Union for ConversationState

The backend sends `ConversationState` with a `type` field. The current type is a flat
interface with all fields optional on all variants. This is replaced with a discriminated
union where fields only exist on variants where they are meaningful:

```typescript
export type ConversationState =
  | { type: 'idle' }
  | { type: 'awaiting_llm' }
  | { type: 'llm_requesting'; attempt: number }
  | { type: 'tool_executing'; current_tool: ToolCall; remaining_tools: ToolCall[] }
  | { type: 'awaiting_sub_agents'; pending: PendingSubAgent[]; completed_results: SubAgentResult[] }
  | { type: 'awaiting_continuation'; attempt: number }
  | { type: 'cancelling' }
  | { type: 'cancelling_tool'; current_tool: ToolCall }
  | { type: 'cancelling_sub_agents'; pending: PendingSubAgent[] }
  | { type: 'context_exhausted'; summary: string }
  | { type: 'error'; message: string }
  | { type: 'terminal' };
```

`current_tool` only exists on `tool_executing` and `cancelling_tool`. The compiler
rejects `state?.current_tool` from `idle`. Every switch requires `satisfies never` at
the default — new backend variants are compile errors:

```typescript
function isAgentWorking(state: ConversationState): boolean {
  switch (state.type) {
    case 'idle': case 'error': case 'terminal': case 'context_exhausted':
      return false;
    case 'awaiting_llm': case 'llm_requesting': case 'tool_executing':
    case 'awaiting_sub_agents': case 'awaiting_continuation':
    case 'cancelling': case 'cancelling_tool': case 'cancelling_sub_agents':
      return true;
    default: state satisfies never; return false;
  }
}
```

`agentWorking` is no longer a separate `useState` — it is derived from `convState`
via `isAgentWorking()`. One source, one answer.

### Conversation Atom and Reducer

All conversation state lives in a single atom managed by `useReducer`. No independent
`useState` calls for `convState`, `agentWorking`, `breadcrumbs`, `contextWindowUsed`,
`stateData`, or `contextExhaustedSummary`. All are fields in the atom or selectors
derived from it.

```typescript
interface ConversationAtom {
  conversationId: string;
  phase: ConversationState;
  messages: Message[];
  breadcrumbs: Breadcrumb[];
  breadcrumbSequenceIds: ReadonlySet<number>;
  contextWindow: { used: number; total: number; exhaustedSummary: string | null };
  systemPrompt: string | null;
  pendingImages: PendingImage[];
  lastSequenceId: number;
  connectionState: 'connecting' | 'live' | 'reconnecting' | 'failed';
  streamingBuffer: StreamingBuffer | null;
  uiError: UIError | null;
}

interface StreamingBuffer {
  text: string;
  lastSequence: number;
  startedAt: number;
}

type UIError =
  | { type: 'ParseError'; raw: string }
  | { type: 'BackendError'; message: string }
  | { type: 'ConnectionFailed'; retriesExhausted: boolean };
```

The reducer is the single entry point for all SSE events:

```typescript
type SSEAction =
  | { type: 'sse_init'; payload: InitPayload }
  | { type: 'sse_message'; message: Message; sequenceId: number }
  | { type: 'sse_state_change'; phase: ConversationState; sequenceId: number }
  | { type: 'sse_agent_done'; sequenceId: number }
  | { type: 'sse_token'; delta: string; sequence: number }
  | { type: 'sse_error'; error: UIError }
  | { type: 'connection_state'; state: ConversationAtom['connectionState'] };

function conversationReducer(atom: ConversationAtom, action: SSEAction): ConversationAtom {
  switch (action.type) {
    case 'sse_init':
      // Authoritative: replace breadcrumbs entirely, clear streaming buffer
      return { ...atom, ...action.payload, streamingBuffer: null };

    case 'sse_message':
      if (atom.lastSequenceId >= action.sequenceId) return atom; // idempotent
      return {
        ...atom,
        messages: [...atom.messages, action.message],
        lastSequenceId: action.sequenceId,
        streamingBuffer: null, // atomic swap: streaming buffer dies, final message lives
      };

    case 'sse_state_change': {
      if (atom.lastSequenceId >= action.sequenceId) return atom;
      const newCrumb = breadcrumbFromPhase(action.phase, action.sequenceId);
      return {
        ...atom,
        phase: action.phase,
        breadcrumbs: newCrumb && !atom.breadcrumbSequenceIds.has(action.sequenceId)
          ? [...atom.breadcrumbs, newCrumb]
          : atom.breadcrumbs,
        breadcrumbSequenceIds: newCrumb
          ? new Set([...atom.breadcrumbSequenceIds, action.sequenceId])
          : atom.breadcrumbSequenceIds,
        lastSequenceId: action.sequenceId,
      };
    }

    case 'sse_token':
      if (atom.streamingBuffer && atom.streamingBuffer.lastSequence >= action.sequence)
        return atom;
      return {
        ...atom,
        streamingBuffer: {
          text: (atom.streamingBuffer?.text ?? '') + action.delta,
          lastSequence: action.sequence,
          startedAt: atom.streamingBuffer?.startedAt ?? Date.now(),
        },
      };

    case 'sse_agent_done':
      if (atom.lastSequenceId >= action.sequenceId) return atom;
      return {
        ...atom,
        phase: { type: 'idle' },
        lastSequenceId: action.sequenceId,
        streamingBuffer: null,
      };

    case 'sse_error':
      return { ...atom, uiError: action.error };

    case 'connection_state':
      return { ...atom, connectionState: action.state };
  }
}
```

**Key invariants enforced by the reducer:**
- Streaming buffer and finalized message cannot both exist — `sse_message` clears the
  buffer atomically in one reducer call, producing one React render (REQ-UI-019).
- `init` replaces breadcrumbs entirely. `state_change` appends with `sequenceId` dedup.
  No external `updateBreadcrumbsFromState()` mutation path.
- Idempotency via `lastSequenceId >= event.sequenceId` — O(1) check, replaces
  unbounded `seenIdsRef: Set<number>`.
- Malformed SSE events become typed `UIError` values, not unhandled exceptions.

### Router-Level Context (REQ-UI-020)

The atom lives in a React context mounted at the router level, above individual page
components. `lastSequenceId` survives navigation. On mount with the same
`conversationId`, SSE reconnects from `atom.lastSequenceId` without re-fetching:

```typescript
function ConversationProvider({ children }: { children: React.ReactNode }) {
  const [atoms, setAtoms] = useState<Map<string, ConversationAtom>>(new Map());

  function getOrCreateAtom(conversationId: string): [ConversationAtom, Dispatch<SSEAction>] {
    // Returns existing atom if present (navigation back)
    // Creates fresh atom if new conversation
  }

  return (
    <ConversationContext.Provider value={{ getOrCreateAtom }}>
      {children}
    </ConversationContext.Provider>
  );
}
```

Components receive selector outputs, never the raw atom:

```typescript
function useConversationSelectors(convId: string) {
  const [atom, dispatch] = useConversationAtom(convId);
  return {
    isAgentWorking: isAgentWorking(atom.phase),
    currentTool: selectCurrentTool(atom),
    streamingText: atom.streamingBuffer?.text ?? null,
    contextWarning: selectContextWarning(atom),
    breadcrumbs: atom.breadcrumbs,
    dispatch,
  };
}
```

### Wiring `appMachine.ts` (existing dead FSM)

`appMachine.ts` defines a correct pure FSM for online/offline/sync state. It is never
instantiated. `useAppMachine.ts` reimplements the same behavior with ad-hoc `useState`.
Fix: replace `useAppMachine.ts` internals to call `appMachine.ts`'s `transition()`,
keeping the public interface unchanged — zero call-site changes:

```typescript
export function useAppMachine(): AppMachineHandle {
  const [appState, setAppState] = useState<AppState>(initialAppState);

  const dispatch = useCallback((event: AppEvent) => {
    setAppState(current => {
      const { state, effects } = transition(current, event, context);
      setTimeout(() => executeEffects(effects), 0);
      return state;
    });
  }, []);

  return {
    isReady: appState.type === 'ready',
    isOnline: appState.type === 'ready' ? appState.network === 'online' : navigator.onLine,
    isSyncing: appState.type === 'ready' && appState.sync.type === 'syncing',
    pendingOpsCount: pendingOpsCountRef.current,
    queueOperation,
  };
}
```

After this: `appMachine.ts` is testable by importing `transition` and feeding events.
No React, no DOM. One running implementation matching its spec.

## Token Streaming Display (REQ-UI-019, REQ-BED-025)

### StreamingState

Token events accumulate in `streamingBuffer` on the conversation atom. The display
component renders based on whether the buffer exists:

```typescript
function StreamingMessage({ buffer }: { buffer: StreamingBuffer | null }) {
  if (!buffer) return null;
  return <div className="streaming-message">{buffer.text}</div>;
}
```

The `sse_message` action clears `streamingBuffer` atomically — one reducer call, one
React render. The finalized message and streaming buffer cannot both be visible because
the reducer produces them in the same state update.

### Reconnection During Streaming

On reconnect, missed token events are not replayed. The UI shows "thinking..." (from
`isAgentWorking(phase)`) until either:
- New token events arrive on the fresh connection (streaming resumes from current point)
- The `sse_message` event arrives with the finalized message (streaming was completed)

This is acceptable per REQ-UI-019: the requirement specifies no-flicker on completion,
not lossless token replay.

## Conversation State Indicators (REQ-UI-012)

Each conversation displays its current state. The display state is derived from
`ConversationState` via exhaustive switch — no catch-all:

```typescript
type ConvDisplayState = 'idle' | 'working' | 'error';

function getDisplayState(state: ConversationState): ConvDisplayState {
  switch (state.type) {
    case 'idle': case 'terminal':
      return 'idle';
    case 'error': case 'context_exhausted':
      return 'error';
    case 'awaiting_llm': case 'llm_requesting': case 'tool_executing':
    case 'awaiting_sub_agents': case 'awaiting_continuation':
    case 'cancelling': case 'cancelling_tool': case 'cancelling_sub_agents':
      return 'working';
    default: state satisfies never; return 'idle';
  }
}

function StateIndicator({ state }: { state: ConversationState }) {
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

`lastSequenceId` lives in the conversation atom (router-level context), not in a
component ref. It survives navigation. The unbounded `seenIds: Set<number>` is replaced
by the reducer's `lastSequenceId >= event.sequenceId` idempotency check — O(1) space.

```typescript
function reconnect(atom: ConversationAtom) {
  const url = atom.lastSequenceId > 0
    ? `/api/conversations/${atom.conversationId}/stream?after=${atom.lastSequenceId}`
    : `/api/conversations/${atom.conversationId}/stream`;
  eventSource = new EventSource(url);
}

// Deduplication is in the reducer — no separate seenIds set needed.
// The reducer returns the atom unchanged if lastSequenceId >= event.sequenceId.
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
| `ConversationProvider` | Router-level context, atom lifecycle | REQ-UI-020 |
| `DesktopLayout` | Viewport detection, sidebar wrapper | REQ-UI-010, REQ-UI-016 |
| `Sidebar` | Conversation list, new form, collapse | REQ-UI-016, REQ-UI-018 |
| `ConversationList` | List display, state indicators, selection | REQ-UI-001, REQ-UI-012 |
| `NewConversationSheet` | Mobile bottom sheet | REQ-UI-015 |
| `NewConversationPage` | Full-page form | REQ-UI-017 |
| `SidebarNewForm` | Inline sidebar form | REQ-UI-018 |
| `MessageList` | Message display, scroll memory, streaming | REQ-UI-002, REQ-UI-013, REQ-UI-019 |
| `StreamingMessage` | In-progress token display | REQ-UI-019 |
| `InputArea` | Composition, drafts, queue | REQ-UI-003, REQ-UI-004 |
| `StateBar` | Connection status, context info, state label | REQ-UI-005, REQ-UI-007 |
| `BreadcrumbBar` | Completed + in-progress step trail | REQ-UI-007a |

---

## Appendix A: UI Architecture Review — Failure Modes and Design Decisions

*This appendix preserves the pre-refactor analysis that drove the typed state
architecture. It was produced by commissioning five independent expert proposals after
analysis of all UI-related bugs and architectural weaknesses.*

### The Seven UI Failure Modes

**UI-FM-1: State inferred from string content, not semantic type.**
`ConversationState.type` is `string`. Components match against it with `===` comparisons
and `startsWith()`. A catch-all `else` maps unrecognized states to a generic indicator.
New backend state variants are silently absorbed.
**Prevention:** Discriminated union with `satisfies never` on every switch. New variants
are compile errors.

**UI-FM-2: Dual representation of the same fact.**
`agentWorking: boolean` and `convState: string` both represent "is the agent working?"
Updated by different events. Between two events, they disagree — the send button can
remain disabled while the state says idle.
**Prevention:** `agentWorking` deleted. `isAgentWorking()` is a pure selector of
`convState`. One source, one answer.

**UI-FM-3: SSE event handler has no error boundary.**
`JSON.parse(e.data)` in SSE listeners without try/catch. Malformed events throw
unhandled exceptions. `seenIdsRef: Set<number>` grows without bound across reconnections.
**Prevention:** SSE events parsed into `Result<SSEAction, UIError>`. Errors become typed
values in the atom. `seenIdsRef` replaced by `lastSequenceId >=` idempotency check.

**UI-FM-4: Dead spec creates parallel authority.**
`appMachine.ts` defines a correct pure FSM. `useAppMachine.ts` reimplements the same
behavior with ad-hoc `useState`. Neither imports the other. Bugs fixed in one are not
reflected in the other.
**Prevention:** `useAppMachine.ts` wraps `appMachine.ts`'s `transition()`. One running
implementation, testable in isolation.

**UI-FM-5: Breadcrumbs have two conflicting writers.**
`init` events send server-reconstructed breadcrumbs. `state_change` events trigger
`updateBreadcrumbsFromState()`. No protocol defines which writer owns which portion.
Reconnect mid-conversation can produce duplicates.
**Prevention:** Reducer is the single writer. `init` replaces entirely. `state_change`
appends with `sequenceId` dedup. No external mutation path.

**UI-FM-6: Navigation destroys in-flight state with no handoff.**
All state lives in `ConversationPage` `useState`. Navigation unmounts the component,
discarding `lastSequenceId` and the deduplication set. Return causes full re-sync.
**Prevention:** Atom lives in router-level React context. `lastSequenceId` survives
navigation. Reconnect uses `?after={lastSeq}` immediately.

**UI-FM-7: Display layer receives untyped semantic state.**
`ConversationState` is a flat interface with all fields optional on all variants.
`?.current_tool` is permitted in `idle` state. The compiler cannot distinguish
intentional access from defensive guessing.
**Prevention:** Discriminated union. `current_tool` only exists on `tool_executing` and
`cancelling_tool`. Every `?.current_tool` becomes a compile error.

### Panel Summary and Architecture Selection

| Proposal | Approach | Adopted? |
|----------|----------|----------|
| **Priya Sundaram** (FRP) | Single canonical atom with reducer, selectors derived | Yes — atom + reducer architecture |
| **Konstantin Orel** (SSE Protocol) | Typed protocol with `generation_id`, `token_done`, `reset`; session store | Protocol: deferred (backend work needed). Session concept: yes (router-level context) |
| **Maya Chen** (Transparency) | 14-question transparency contract; StatusBar + BreadcrumbBar separation | Contract: yes (framing section in requirements.md). Separate StatusBar: no (fix existing) |
| **Diego Ramos** (Offline-First) | IndexedDB as authoritative store; write-through cache | No — React context sufficient for navigation persistence. IndexedDB can be retrofitted later |
| **Anya Kowalski** (Type Systems) | Discriminated unions with `satisfies never`; `StreamingState` union | Yes — type foundation for all state |
