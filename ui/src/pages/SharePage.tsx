import { useState, useEffect, useRef, useCallback, type Dispatch } from 'react';
import { useParams } from 'react-router-dom';
import type {
  Conversation,
  Message,
  ConversationState,
  SseInitData,
  SseMessageData,
  SseStateChangeData,
} from '../api';
import { parseConversationState } from '../utils';
import { MessageList } from '../components/MessageList';
import { MessageListSkeleton } from '../components/Skeleton';
import { BreadcrumbBar } from '../components/BreadcrumbBar';
import type { Breadcrumb } from '../types';
import type { SseBreadcrumb } from '../api';
import { parseEvent } from '../hooks/useConnection';
import {
  SseInitDataSchema,
  SseMessageDataSchema,
  SseStateChangeDataSchema,
  SseTokenDataSchema,
} from '../sseSchemas';
import type { SSEAction } from '../conversation/atom';

function transformBreadcrumb(b: SseBreadcrumb): Breadcrumb {
  return {
    type: b.type,
    label: b.label,
    toolId: b.tool_id,
    sequenceId: b.sequence_id,
    preview: b.preview,
  };
}

type ShareStatus = 'loading' | 'connected' | 'error' | 'not_found';

export function SharePage() {
  const { token } = useParams<{ token: string }>();
  const [status, setStatus] = useState<ShareStatus>('loading');
  const [conversation, setConversation] = useState<Conversation | null>(null);
  const [messages, setMessages] = useState<Message[]>([]);
  const [convState, setConvState] = useState<ConversationState>({ type: 'idle' });
  const [breadcrumbs, setBreadcrumbs] = useState<Breadcrumb[]>([]);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const eventSourceRef = useRef<EventSource | null>(null);
  const lastSequenceIdRef = useRef(0);

  const handleSseInit = useCallback((raw: SseInitData) => {
    setConversation(raw.conversation);
    setMessages(raw.messages || []);
    setConvState(parseConversationState(raw.conversation?.state));
    setBreadcrumbs((raw.breadcrumbs || []).map(transformBreadcrumb));
    lastSequenceIdRef.current = raw.last_sequence_id ?? 0;
    setStatus('connected');
  }, []);

  const handleSseMessage = useCallback((data: SseMessageData) => {
    const msg = data.message;
    if (msg.sequence_id <= lastSequenceIdRef.current) return;
    lastSequenceIdRef.current = msg.sequence_id;
    setMessages(prev => {
      const existing = prev.findIndex(m => m.message_id === msg.message_id);
      if (existing >= 0) {
        const updated = [...prev];
        updated[existing] = msg;
        return updated;
      }
      return [...prev, msg];
    });
  }, []);

  const handleSseStateChange = useCallback((data: SseStateChangeData) => {
    setConvState(parseConversationState(data.state));
  }, []);

  useEffect(() => {
    if (!token) {
      setStatus('not_found');
      return;
    }

    // Adapter: `parseEvent` dispatches `SSEAction`s on validation failure. The
    // share view has no reducer — just local error state — so translate the
    // only action `parseEvent` ever dispatches (`sse_error`) into SharePage's
    // error surface. Other SSEAction variants are unreachable here (parseEvent
    // never emits them), so ignoring them is structurally safe.
    const shareDispatch: Dispatch<SSEAction> = (action) => {
      if (action.type !== 'sse_error') return;
      setStatus('error');
      const err = action.error;
      if (err.type === 'ParseError') {
        setErrorMessage('Failed to parse server data');
      } else if (err.type === 'BackendError') {
        setErrorMessage(err.message);
      } else {
        setErrorMessage('Connection to share stream failed');
      }
    };

    // Connect to shared SSE stream
    const url = `/api/share/${encodeURIComponent(token)}/events`;
    const es = new EventSource(url);
    eventSourceRef.current = es;

    es.addEventListener('init', (e) => {
      const res = parseEvent(SseInitDataSchema, e, 'init', shareDispatch);
      if (!res.ok) return;
      handleSseInit(res.data);
    });

    es.addEventListener('message', (e) => {
      const res = parseEvent(SseMessageDataSchema, e, 'message', shareDispatch);
      if (!res.ok) return;
      handleSseMessage(res.data);
    });

    es.addEventListener('state_change', (e) => {
      const res = parseEvent(SseStateChangeDataSchema, e, 'state_change', shareDispatch);
      if (!res.ok) return;
      handleSseStateChange(res.data);
    });

    es.addEventListener('token', (e) => {
      // Read-only mode: we don't render streaming deltas; wait for the
      // full message event. The handler still validates the wire shape so
      // a future server-side change surfaces as a schema error instead of
      // silent drift.
      parseEvent(SseTokenDataSchema, e, 'token', shareDispatch);
    });

    es.addEventListener('error', () => {
      if (es.readyState === EventSource.CLOSED) {
        setStatus('error');
        setErrorMessage('Connection to share stream closed');
      }
    });

    es.onerror = () => {
      // EventSource auto-reconnects, but if the token is invalid the server returns 404
      // which causes the connection to fail immediately
      if (es.readyState === EventSource.CLOSED) {
        setStatus('not_found');
      }
    };

    return () => {
      es.close();
      eventSourceRef.current = null;
    };
  }, [token, handleSseInit, handleSseMessage, handleSseStateChange]);

  if (status === 'not_found') {
    return (
      <div className="share-page">
        <div className="share-banner share-banner--error">
          Share link not found or has been revoked
        </div>
      </div>
    );
  }

  if (status === 'error') {
    return (
      <div className="share-page">
        <div className="share-banner share-banner--error">
          {errorMessage || 'Failed to connect to share stream'}
        </div>
      </div>
    );
  }

  if (status === 'loading' || !conversation) {
    return (
      <div className="share-page">
        <div className="share-banner">Connecting to shared conversation...</div>
        <main className="share-main">
          <section className="share-chat-view">
            <div className="share-messages">
              <MessageListSkeleton count={4} />
            </div>
          </section>
        </main>
      </div>
    );
  }

  const slug = conversation.slug || 'conversation';
  const model = conversation.model || '';
  const modeLabel = conversation.conv_mode_label || '';

  return (
    <div className="share-page">
      <div className="share-banner">
        Read-only shared view
        <span className="share-banner-meta">
          {slug}
          {modeLabel ? ` / ${modeLabel}` : ''}
          {model ? ` / ${model}` : ''}
        </span>
      </div>
      <main className="share-main">
        <MessageList
          messages={messages}
          queuedMessages={[]}
          convState={convState}
          onRetry={() => {}}
          onOpenFile={undefined}
          conversationId={conversation.id}
          streamingBuffer={null}
        />
        <BreadcrumbBar breadcrumbs={breadcrumbs} visible={breadcrumbs.length > 0} />
      </main>
    </div>
  );
}
