import { useState, useEffect, useRef, useCallback } from 'react';
import { useParams, useNavigate } from 'react-router-dom';
import { api, Conversation, Message, ConversationState, SseEventType, SseEventData, SseInitData, SseMessageData, SseStateChangeData } from '../api';
import { StateBar } from '../components/StateBar';
import { BreadcrumbBar } from '../components/BreadcrumbBar';
import { MessageList } from '../components/MessageList';
import { InputArea } from '../components/InputArea';
import { useDraft, useMessageQueue, useConnection } from '../hooks';
import type { Breadcrumb } from '../types';

export function ConversationPage() {
  const { slug } = useParams<{ slug: string }>();
  const navigate = useNavigate();

  const [conversationId, setConversationId] = useState<string | undefined>(undefined);
  const [conversation, setConversation] = useState<Conversation | null>(null);
  const [messages, setMessages] = useState<Message[]>([]);
  const [convState, setConvState] = useState('idle');
  const [stateData, setStateData] = useState<ConversationState | null>(null);
  const [breadcrumbs, setBreadcrumbs] = useState<Breadcrumb[]>([]);
  const [agentWorking, setAgentWorking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [contextWindowUsed, setContextWindowUsed] = useState(0);

  const sendingMessagesRef = useRef<Set<string>>(new Set()); // Track localIds being sent

  // Draft management
  const [draft, setDraft, clearDraft] = useDraft(conversationId);

  // Message queue management
  const { queuedMessages, enqueue, markSent, markFailed, retry } = useMessageQueue(conversationId);

  // Update breadcrumbs from state
  const updateBreadcrumbsFromState = useCallback((state: string, data: ConversationState | null) => {
    if (state === 'idle' || state === 'error') {
      return;
    }

    setBreadcrumbs((prev) => {
      const updated = [...prev];

      if (state === 'llm_requesting') {
        const filtered = updated.filter((b) => b.type !== 'llm');
        const attempt = data?.attempt || 1;
        const label = attempt > 1 ? `LLM (retry ${attempt})` : 'LLM';
        filtered.push({ type: 'llm', label });
        return filtered;
      }

      if (state === 'tool_executing' && data?.current_tool) {
        const toolName = data.current_tool.input?._tool || 'tool';
        const toolId = data.current_tool.id;
        const remaining = data.remaining_tools?.length ?? 0;
        const label = remaining > 0 ? `${toolName} (+${remaining})` : toolName;

        if (!updated.some((b) => b.type === 'tool' && b.toolId === toolId)) {
          updated.push({ type: 'tool', label, toolId });
        }
        return updated;
      }

      if (state === 'awaiting_sub_agents') {
        const pending = data?.pending_ids?.length ?? 0;
        const completed = data?.completed_results?.length ?? 0;
        const total = pending + completed;
        const label = `sub-agents (${completed}/${total})`;

        const existing = updated.find((b) => b.type === 'subagents');
        if (existing) {
          existing.label = label;
        } else {
          updated.push({ type: 'subagents', label });
        }
        return updated;
      }

      return updated;
    });
  }, []);

  // Handle SSE events
  const handleSseEvent = useCallback(
    (eventType: SseEventType, data: SseEventData) => {
      switch (eventType) {
        case 'init': {
          const initData = data as SseInitData;
          setConversation(initData.conversation);
          setMessages(initData.messages || []);
          const initState = initData.conversation?.state || { type: 'idle' };
          setConvState(initState.type || 'idle');
          const { type: _, ...initStateData } = initState;
          setStateData(Object.keys(initStateData).length > 0 ? initStateData as ConversationState : null);
          setAgentWorking(initData.agent_working || false);
          if (initData.context_window_size !== undefined) {
            setContextWindowUsed(initData.context_window_size);
          }
          updateBreadcrumbsFromState(initState.type || 'idle', initStateData as ConversationState);
          break;
        }

        case 'message': {
          const msgData = data as SseMessageData;
          const msg = msgData.message;
          if (msg) {
            setMessages((prev) => {
              // Deduplicate by sequence_id
              if (prev.some(m => m.sequence_id === msg.sequence_id)) {
                return prev;
              }
              return [...prev, msg];
            });
            // Update context window usage from message usage data
            if (msg.usage_data) {
              const usage = msg.usage_data;
              const msgTokens = (usage.input_tokens || 0) + (usage.output_tokens || 0) +
                (usage.cache_creation_input_tokens || 0) + (usage.cache_read_input_tokens || 0);
              if (msgTokens > 0) {
                setContextWindowUsed(prev => prev + msgTokens);
              }
            }
            // New user message = new turn, reset breadcrumbs
            if (msg.message_type === 'user' || msg.type === 'user') {
              setBreadcrumbs([{ type: 'user', label: 'User' }]);
            }
          }
          break;
        }

        case 'state_change': {
          const stateChangeData = data as SseStateChangeData;
          const newState = stateChangeData.state?.type || 'idle';
          const { type, ...rest } = stateChangeData.state || { type: 'idle' };
          setConvState(newState);
          setStateData(Object.keys(rest).length > 0 ? rest as ConversationState : null);
          setAgentWorking(!['idle', 'error', 'completed', 'failed'].includes(newState));
          updateBreadcrumbsFromState(newState, rest as ConversationState);
          break;
        }

        case 'agent_done':
          setAgentWorking(false);
          setConvState('idle');
          setBreadcrumbs([]);
          break;

        case 'disconnected':
          // Connection hook handles reconnection, no action needed here
          break;
      }
    },
    [updateBreadcrumbsFromState]
  );

  // Connection management with automatic reconnection
  const connectionInfo = useConnection({
    conversationId,
    onEvent: handleSseEvent,
  });

  const isOffline = connectionInfo.state === 'offline' || connectionInfo.state === 'reconnecting';
  const isConnected = connectionInfo.state === 'connected' || connectionInfo.state === 'reconnected';

  // Load conversation by slug (just to get the ID)
  useEffect(() => {
    if (!slug) {
      navigate('/');
      return;
    }

    // Reset state for new conversation
    setConversationId(undefined);
    setConversation(null);
    setMessages([]);
    setError(null);

    let cancelled = false;

    const loadConversation = async () => {
      try {
        const result = await api.getConversationBySlug(slug);
        if (cancelled) return;
        
        // Set initial data from REST API
        setConversation(result.conversation);
        setMessages(result.messages);
        setAgentWorking(result.agent_working);
        setContextWindowUsed(result.context_window_size || 0);
        
        // Set conversation ID for hooks - this triggers the SSE connection
        setConversationId(result.conversation.id);
      } catch (err) {
        if (cancelled) return;
        console.error('Failed to load conversation:', err);
        setError(err instanceof Error ? err.message : 'Failed to load conversation');
      }
    };

    loadConversation();

    return () => {
      cancelled = true;
    };
  }, [slug, navigate]);

  // Refs for queue callbacks to avoid effect re-runs
  const markSentRef = useRef(markSent);
  const markFailedRef = useRef(markFailed);
  useEffect(() => { markSentRef.current = markSent; }, [markSent]);
  useEffect(() => { markFailedRef.current = markFailed; }, [markFailed]);

  // Send a message (either new or retry)
  const sendMessage = useCallback(async (localId: string, text: string, images: { data: string; media_type: string }[] = []) => {
    if (!conversationId) return;

    // Mark as being sent
    sendingMessagesRef.current.add(localId);

    try {
      await api.sendMessage(conversationId, text, images);
      markSentRef.current(localId);
      setAgentWorking(true);
      setBreadcrumbs([{ type: 'user', label: 'User' }]);
    } catch (err) {
      console.error('Failed to send message:', err);
      markFailedRef.current(localId);
    } finally {
      sendingMessagesRef.current.delete(localId);
    }
  }, [conversationId]); // Only depends on conversationId - callbacks accessed via refs

  // Stable ref to sendMessage for effect
  const sendMessageRef = useRef(sendMessage);
  useEffect(() => { sendMessageRef.current = sendMessage; }, [sendMessage]);

  // Send queued messages when connection is restored
  useEffect(() => {
    if (!isConnected || !conversationId) return;

    const pending = queuedMessages.filter(
      m => m.status === 'sending' && !sendingMessagesRef.current.has(m.localId)
    );

    for (const msg of pending) {
      sendMessageRef.current(msg.localId, msg.text, msg.images);
    }
  }, [isConnected, conversationId, queuedMessages]); // sendMessage accessed via ref

  // Handle send from input
  const handleSend = (text: string) => {
    if (!conversationId) return;

    // Clear draft
    clearDraft();

    // Enqueue the message (shows immediately with sending state)
    const msg = enqueue(text);

    // If we're connected, send immediately
    if (isConnected) {
      sendMessage(msg.localId, text);
    }
    // If offline, message will be sent when connection is restored (via useEffect above)
  };

  // Handle retry
  const handleRetry = (localId: string) => {
    const msg = queuedMessages.find(m => m.localId === localId);
    if (!msg) return;

    retry(localId);
    
    if (isConnected) {
      sendMessage(localId, msg.text, msg.images);
    }
  };

  // Cancel operation
  const handleCancel = async () => {
    if (!conversationId || !agentWorking) return;
    if (convState.startsWith('cancelling')) return;

    try {
      await api.cancelConversation(conversationId);
    } catch (err) {
      console.error('Failed to cancel:', err);
    }
  };

  if (error) {
    return (
      <div id="app">
        <StateBar
          conversation={null}
          convState="error"
          stateData={null}
          connectionState="disconnected"
          connectionAttempt={0}
          nextRetryIn={null}
          contextWindowUsed={0}
        />
        <BreadcrumbBar breadcrumbs={[]} visible={false} />
        <main id="main-area">
          <div className="empty-state">
            <div className="empty-state-icon">❌</div>
            <p>{error}</p>
            <button className="btn-primary" onClick={() => navigate('/')} style={{ marginTop: 16 }}>
              Back to List
            </button>
          </div>
        </main>
      </div>
    );
  }

  if (!conversation) {
    return (
      <div id="app">
        <StateBar
          conversation={null}
          convState="idle"
          stateData={null}
          connectionState="connecting"
          connectionAttempt={0}
          nextRetryIn={null}
          contextWindowUsed={0}
        />
        <BreadcrumbBar breadcrumbs={[]} visible={false} />
        <main id="main-area">
          <div className="empty-state">
            <div className="spinner"></div>
            <p>Loading...</p>
          </div>
        </main>
      </div>
    );
  }

  const canSend = !agentWorking;
  const isCancelling = convState.startsWith('cancelling');

  return (
    <div id="app">
      <StateBar
        conversation={conversation}
        convState={convState}
        stateData={stateData}
        connectionState={connectionInfo.state}
        connectionAttempt={connectionInfo.attempt}
        nextRetryIn={connectionInfo.nextRetryIn}
        contextWindowUsed={contextWindowUsed}
      />
      <BreadcrumbBar breadcrumbs={breadcrumbs} visible={true} />
      <MessageList
        messages={messages}
        queuedMessages={queuedMessages}
        convState={convState}
        stateData={stateData}
        onRetry={handleRetry}
      />
      <InputArea
        draft={draft}
        setDraft={setDraft}
        canSend={canSend}
        agentWorking={agentWorking}
        isCancelling={isCancelling}
        isOffline={isOffline}
        queuedMessages={queuedMessages}
        onSend={handleSend}
        onCancel={handleCancel}
        onRetry={handleRetry}
      />
    </div>
  );
}
