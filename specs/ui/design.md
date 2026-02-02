# Web UI Design

## Technology Stack

- **React 18** with TypeScript
- **React Router v6** for client-side routing
- **Vite** for development and building
- **CSS Variables** for theming (no CSS framework)
- **localStorage** for persistence

## Architecture

```
ui/
  src/
    api.ts              # API client and types
    utils.ts            # Shared utilities
    types.ts            # Shared types
    App.tsx             # Router setup
    index.css           # Global styles
    pages/
      ConversationListPage.tsx
      ConversationPage.tsx
    components/
      StateBar.tsx      # Connection status header
      BreadcrumbBar.tsx # Activity breadcrumbs
      MessageList.tsx   # Message display
      InputArea.tsx     # Message composition
      ConversationList.tsx
      NewConversationModal.tsx
    hooks/
      useLocalStorage.ts
      useConnection.ts
      usePendingMessages.ts
```

## Message Delivery State Machine (REQ-UI-004)

```
[draft] --send--> [sending] --success--> [sent] ✓
                      |
                      +--error--> [failed] --retry--> [sending]
```

When offline, messages are queued internally but still show as "sending" to the user.
The queue is persisted to localStorage and flushed when connection is restored.

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

### Reconnection Backoff

```typescript
const BACKOFF_BASE = 1000;      // 1 second
const BACKOFF_MAX = 30000;      // 30 seconds
const OFFLINE_THRESHOLD = 3;    // Show "offline" after N failures

function getBackoffDelay(attempt: number): number {
  return Math.min(BACKOFF_BASE * Math.pow(2, attempt - 1), BACKOFF_MAX);
}

// Attempt 1: 1s
// Attempt 2: 2s
// Attempt 3: 4s (show offline banner)
// Attempt 4: 8s
// Attempt 5: 16s
// Attempt 6+: 30s
```

### Reconnection with Sequence Tracking

```typescript
// On each message event
function handleMessage(msg: Message) {
  lastSequenceId.current = msg.sequence_id;
  localStorage.setItem(`phoenix:lastSeq:${convId}`, String(msg.sequence_id));
  // ... update state
}

// On reconnect
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
  if (seenIds.has(msg.sequence_id)) return; // Already have this one
  seenIds.add(msg.sequence_id);
  // ... process message
}
```

## Message Queue (REQ-UI-004)

```typescript
type MessageStatus = 'sending' | 'failed';

interface QueuedMessage {
  localId: string;      // UUID generated client-side
  text: string;
  images: ImageData[];
  timestamp: number;
  status: MessageStatus;
}

class MessageQueue {
  private messages: QueuedMessage[] = [];
  private storageKey: string;
  
  constructor(conversationId: string) {
    this.storageKey = `phoenix:queue:${conversationId}`;
    this.load();
  }
  
  private load() {
    const stored = localStorage.getItem(this.storageKey);
    this.messages = stored ? JSON.parse(stored) : [];
  }
  
  private save() {
    if (this.messages.length === 0) {
      localStorage.removeItem(this.storageKey);
    } else {
      localStorage.setItem(this.storageKey, JSON.stringify(this.messages));
    }
  }
  
  /** Add a new message to the queue */
  enqueue(text: string, images: ImageData[] = []): QueuedMessage {
    const msg: QueuedMessage = {
      localId: crypto.randomUUID(),
      text,
      images,
      timestamp: Date.now(),
      status: 'sending'
    };
    this.messages.push(msg);
    this.save();
    return msg;
  }
  
  /** Remove a message (on successful send) */
  remove(localId: string) {
    this.messages = this.messages.filter(m => m.localId !== localId);
    this.save();
  }
  
  /** Mark a message as failed */
  markFailed(localId: string) {
    const msg = this.messages.find(m => m.localId === localId);
    if (msg) {
      msg.status = 'failed';
      this.save();
    }
  }
  
  /** Mark a failed message as sending (retry) */
  markSending(localId: string) {
    const msg = this.messages.find(m => m.localId === localId);
    if (msg) {
      msg.status = 'sending';
      this.save();
    }
  }
  
  /** Get all queued messages */
  getAll(): QueuedMessage[] {
    return [...this.messages];
  }
  
  /** Get messages that need to be sent */
  getPending(): QueuedMessage[] {
    return this.messages.filter(m => m.status === 'sending');
  }
}
```

## Component Responsibilities

### ConnectionManager (new hook: useConnection)

Manages SSE connection lifecycle:
- Connect/reconnect with backoff
- Track connection state
- Integrate with `navigator.onLine`
- Emit events for state changes
- Track `lastSequenceId`

### MessageQueue (new hook: usePendingMessages)

Manages offline message queue:
- Persist to localStorage
- Expose pending messages for UI
- Flush on reconnection
- Handle send failures

### InputArea (enhanced)

- Save draft to localStorage on change
- Restore draft on mount
- Enqueue to MessageQueue if offline
- Show pending/sending/failed states

### StateBar (enhanced)

- Show reconnection attempt count
- Show offline banner with countdown
- Respond to navigator.onLine

## Visual States Reference

### Connection Indicator

| State | Dot | Text | Banner |
|-------|-----|------|--------|
| connected | 🟢 | "ready" | none |
| connecting | ⚪ | "connecting..." | none |
| reconnecting | 🟡 | "reconnecting (3)..." | none |
| offline | 🔴 | "offline" | "Reconnecting in 8s..." |

### Message States

| State | Icon | Interactive |
|-------|------|-------------|
| draft | (none) | editable in input |
| sending | ⏳ | non-interactive |
| sent | ✓ | normal message |
| failed | ⚠️ | tap to retry |
