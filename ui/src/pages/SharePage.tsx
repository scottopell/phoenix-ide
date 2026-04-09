import { useState, useEffect, useRef, useCallback } from 'react';
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

    // Connect to shared SSE stream
    const url = `/api/share/${encodeURIComponent(token)}/events`;
    const es = new EventSource(url);
    eventSourceRef.current = es;

    es.addEventListener('init', (e) => {
      try {
        const raw = JSON.parse((e as MessageEvent).data) as SseInitData;
        handleSseInit(raw);
      } catch {
        setStatus('error');
        setErrorMessage('Failed to parse server data');
      }
    });

    es.addEventListener('message', (e) => {
      try {
        const data = JSON.parse((e as MessageEvent).data) as SseMessageData;
        handleSseMessage(data);
      } catch {
        // Ignore parse errors for individual messages
      }
    });

    es.addEventListener('state_change', (e) => {
      try {
        const data = JSON.parse((e as MessageEvent).data) as SseStateChangeData;
        handleSseStateChange(data);
      } catch {
        // Ignore parse errors
      }
    });

    es.addEventListener('token', (e) => {
      try {
        const data = JSON.parse((e as MessageEvent).data) as { text: string; request_id?: string };
        if (data.text) {
          setMessages(prev => {
            // Append streaming token to the last agent message, or create a virtual one
            // For simplicity in read-only mode, we just wait for the full message event
            return prev;
          });
        }
      } catch {
        // Ignore
      }
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
