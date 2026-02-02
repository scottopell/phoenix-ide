import { useState, useEffect, useRef, useCallback } from 'react';
import { useParams, useNavigate } from 'react-router-dom';
import { api, Conversation, Message, ConversationState, SseEventType, SseEventData, SseInitData, SseMessageData, SseStateChangeData } from '../api';
import { StateBar } from '../components/StateBar';
import { BreadcrumbBar } from '../components/BreadcrumbBar';
import { MessageList } from '../components/MessageList';
import { InputArea } from '../components/InputArea';
import type { Breadcrumb } from '../types';

export function ConversationPage() {
  const { slug } = useParams<{ slug: string }>();
  const navigate = useNavigate();

  const [conversation, setConversation] = useState<Conversation | null>(null);
  const [messages, setMessages] = useState<Message[]>([]);
  const [convState, setConvState] = useState('idle');
  const [stateData, setStateData] = useState<ConversationState | null>(null);
  const [breadcrumbs, setBreadcrumbs] = useState<Breadcrumb[]>([]);
  const [agentWorking, setAgentWorking] = useState(false);
  const [eventSourceReady, setEventSourceReady] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const eventSourceRef = useRef<EventSource | null>(null);
  const conversationIdRef = useRef<string | null>(null);

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

  // Handle SSE events - stable callback using refs
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
          setEventSourceReady(true);
          updateBreadcrumbsFromState(initState.type || 'idle', initStateData as ConversationState);
          break;
        }

        case 'message': {
          const msgData = data as SseMessageData;
          const msg = msgData.message;
          if (msg) {
            setMessages((prev) => [...prev, msg]);
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
          setEventSourceReady(false);
          // Try to reconnect after a delay
          setTimeout(() => {
            const convId = conversationIdRef.current;
            if (convId) {
              connectToConversation(convId);
            }
          }, 2000);
          break;
      }
    },
    [updateBreadcrumbsFromState]
  );

  // Connect to SSE stream - stable callback
  const connectToConversation = useCallback(
    (convId: string) => {
      // Close existing connection
      if (eventSourceRef.current) {
        eventSourceRef.current.close();
      }
      
      conversationIdRef.current = convId;
      setEventSourceReady(false);

      const es = api.streamConversation(convId, handleSseEvent);
      eventSourceRef.current = es;
    },
    [handleSseEvent]
  );

  // Load conversation by slug
  useEffect(() => {
    if (!slug) {
      navigate('/');
      return;
    }

    let cancelled = false;

    const loadConversation = async () => {
      try {
        const result = await api.getConversationBySlug(slug);
        if (cancelled) return;
        
        // Set initial data from REST API while SSE connects
        setConversation(result.conversation);
        setMessages(result.messages);
        setAgentWorking(result.agent_working);
        
        // Connect to SSE for live updates
        connectToConversation(result.conversation.id);
      } catch (err) {
        if (cancelled) return;
        console.error('Failed to load conversation:', err);
        setError(err instanceof Error ? err.message : 'Failed to load conversation');
      }
    };

    loadConversation();

    // Cleanup on unmount
    return () => {
      cancelled = true;
      if (eventSourceRef.current) {
        eventSourceRef.current.close();
      }
    };
  }, [slug, navigate, connectToConversation]);

  // Send message
  const handleSend = async (text: string) => {
    if (!conversation || agentWorking) return;

    setAgentWorking(true);
    setBreadcrumbs([{ type: 'user', label: 'User' }]);

    try {
      await api.sendMessage(conversation.id, text);
    } catch (err) {
      console.error('Failed to send message:', err);
      setAgentWorking(false);
    }
  };

  // Cancel operation
  const handleCancel = async () => {
    if (!conversation || !agentWorking) return;
    if (convState.startsWith('cancelling')) return;

    try {
      await api.cancelConversation(conversation.id);
    } catch (err) {
      console.error('Failed to cancel:', err);
    }
  };

  if (error) {
    return (
      <div id="app">
        <StateBar conversation={null} convState="error" stateData={null} eventSourceReady={false} />
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
        <StateBar conversation={null} convState="idle" stateData={null} eventSourceReady={false} />
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
        eventSourceReady={eventSourceReady}
      />
      <BreadcrumbBar breadcrumbs={breadcrumbs} visible={true} />
      <MessageList messages={messages} convState={convState} stateData={stateData} />
      <InputArea
        canSend={canSend}
        agentWorking={agentWorking}
        isCancelling={isCancelling}
        onSend={handleSend}
        onCancel={handleCancel}
      />
    </div>
  );
}
