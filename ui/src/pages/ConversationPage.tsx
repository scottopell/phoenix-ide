import { useState, useEffect, useRef, useCallback } from 'react';
import { useParams, useNavigate } from 'react-router-dom';
import { api, Conversation, Message, ConversationState, SseEventType, SseEventData, SseInitData, SseMessageData, SseStateChangeData, ImageData } from '../api';
import { cacheDB } from '../cache';
import { MessageList } from '../components/MessageList';
import { InputArea } from '../components/InputArea';
import { MessageListSkeleton } from '../components/Skeleton';
import { FileBrowser } from '../components/FileBrowser';
import { ProseReader } from '../components/ProseReader';
import { useDraft, useMessageQueue, useConnection } from '../hooks';
import { useAppMachine } from '../hooks/useAppMachine';
import { StateBar } from '../components/StateBar';
import { BreadcrumbBar } from '../components/BreadcrumbBar';
import { ErrorBanner } from '../components/ErrorBanner';
import type { Breadcrumb } from '../types';

export function ConversationPage() {
  const { slug } = useParams<{ slug: string }>();
  const navigate = useNavigate();

  const [conversationId, setConversationId] = useState<string | undefined>(undefined);
  const [conversationIdForSSE, setConversationIdForSSE] = useState<string | undefined>(undefined);
  const [conversation, setConversation] = useState<Conversation | null>(null);
  const [messages, setMessages] = useState<Message[]>([]);
  const [convState, setConvState] = useState('idle');
  const [stateData, setStateData] = useState<ConversationState | null>(null);
  const [breadcrumbs, setBreadcrumbs] = useState<Breadcrumb[]>([]);
  const [agentWorking, setAgentWorking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [contextWindowUsed, setContextWindowUsed] = useState(0);
  const [modelContextWindow, setModelContextWindow] = useState(200_000); // Default fallback
  const [contextExhaustedSummary, setContextExhaustedSummary] = useState<string | null>(null);

  
  // File browser and prose reader state
  const [showFileBrowser, setShowFileBrowser] = useState(false);
  const [proseReaderFile, setProseReaderFile] = useState<{
    path: string;
    rootDir: string;
    patchContext?: {
      modifiedLines: Set<number>;
      firstModifiedLine?: number;
    };
  } | null>(null);

  const sendingMessagesRef = useRef<Set<string>>(new Set());

  // App state for offline support
  const { isOnline, queueOperation } = useAppMachine();

  // Draft management
  const [draft, setDraft, clearDraft] = useDraft(conversationId);
  
  // Image attachments (not persisted - cleared on page refresh)
  const [images, setImages] = useState<ImageData[]>([]);

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
        const pending = data?.pending?.length ?? 0;
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
          
          // On reconnection, we request ?after=lastSeqId and get only NEW messages.
          const newMessages = initData.messages || [];
          setMessages((prev) => {
            if (prev.length === 0) {
              return newMessages;
            }
            // Reconnection - append new messages, deduplicating by sequence_id
            const existingIds = new Set(prev.map(m => m.sequence_id));
            const toAdd = newMessages.filter(m => !existingIds.has(m.sequence_id));
            return toAdd.length > 0 ? [...prev, ...toAdd] : prev;
          });
          
          const initState = initData.conversation?.state || { type: 'idle' };
          setConvState(initState.type || 'idle');
          const { type: _initType, ...initStateData } = initState;
          void _initType; // Destructured to extract remaining state data
          setStateData(Object.keys(initStateData).length > 0 ? initStateData as ConversationState : null);
          setAgentWorking(initData.agent_working || false);
          
          // Initialize context exhausted summary if loading an exhausted conversation
          if (initState.type === 'context_exhausted' && 'summary' in initState) {
            setContextExhaustedSummary((initState as { summary?: string }).summary || null);
          }
          if (initData.context_window_size !== undefined) {
            setContextWindowUsed(initData.context_window_size);
          }
          if (initData.model_context_window !== undefined) {
            setModelContextWindow(initData.model_context_window);
          }
          // Set breadcrumbs from server (reconstructed from message history)
          if (initData.breadcrumbs && initData.breadcrumbs.length > 0) {
            setBreadcrumbs(initData.breadcrumbs.map(b => ({
              type: b.type,
              label: b.label,
              toolId: b.tool_id as string | undefined,
              sequenceId: b.sequence_id as number | undefined,
              preview: b.preview as string | undefined,
            })));
          }
          // Also update from state if agent is still working
          if (initData.agent_working) {
            updateBreadcrumbsFromState(initState.type || 'idle', initStateData as ConversationState);
          }
          
          // Update cache with fresh data from SSE
          if (initData.conversation) {
            cacheDB.putConversation(initData.conversation);
          }
          if (newMessages.length > 0) {
            cacheDB.putMessages(newMessages);
          }
          break;
        }

        case 'message': {
          const msgData = data as SseMessageData;
          const msg = msgData.message;
          if (msg) {
            setMessages((prev) => {
              // Check if message already exists (by message_id)
              const existingIdx = prev.findIndex(m => m.message_id === msg.message_id);
              if (existingIdx >= 0) {
                // Update existing message (e.g., display_data updated)
                const updated = [...prev];
                updated[existingIdx] = msg;
                return updated;
              }
              // Check by sequence_id as fallback (shouldn't happen but safety)
              if (prev.some(m => m.sequence_id === msg.sequence_id)) {
                return prev;
              }
              return [...prev, msg];
            });
            // Update cache with new/updated message
            cacheDB.putMessage(msg);
            
            if (msg.message_type === 'user' || msg.type === 'user') {
              setBreadcrumbs([{ type: 'user', label: 'User' }]);
            }
          }
          break;
        }

        case 'state_change': {
          const stateChangeData = data as SseStateChangeData;
          const newState = stateChangeData.state?.type || 'idle';
          const { type: _stateType, ...rest } = stateChangeData.state || { type: 'idle' };
          void _stateType; // Used newState above instead
          setConvState(newState);
          setStateData(Object.keys(rest).length > 0 ? rest as ConversationState : null);
          setAgentWorking(!['idle', 'error', 'completed', 'failed', 'context_exhausted'].includes(newState));
          updateBreadcrumbsFromState(newState, rest as ConversationState);
          
          // Handle context exhaustion (REQ-BED-021)
          if (newState === 'context_exhausted' && 'summary' in stateChangeData.state) {
            setContextExhaustedSummary((stateChangeData.state as { summary?: string }).summary || null);
          }
          break;
        }

        case 'agent_done':
          setAgentWorking(false);
          setConvState('idle');
          // Keep breadcrumbs visible to show what happened - they clear on next user message
          break;

        case 'disconnected':
          break;
      }
    },
    [updateBreadcrumbsFromState]
  );

  // Connection management with automatic reconnection
  const connectionInfo = useConnection({
    conversationId: conversationIdForSSE,
    onEvent: handleSseEvent,
  });
  
  // Defer SSE connection to not block initial render
  useEffect(() => {
    if (!conversationId) return;
    
    const timer = setTimeout(() => {
      setConversationIdForSSE(conversationId);
    }, 100);
    
    return () => {
      clearTimeout(timer);
      setConversationIdForSSE(undefined);
    };
  }, [conversationId]);

  const isOffline = connectionInfo.state === 'offline' || connectionInfo.state === 'reconnecting';
  const isConnected = connectionInfo.state === 'connected' || connectionInfo.state === 'reconnected';

  // Load conversation by slug: cache first, then SSE provides fresh data
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
        // Step 1: Show cached data immediately
        const cached = await cacheDB.getConversationBySlug(slug);
        if (cached && !cancelled) {
          setConversation(cached);
          const cachedMessages = await cacheDB.getMessages(cached.id);
          setMessages(cachedMessages);
          setConversationId(cached.id); // Triggers SSE connection
          return;
        }

        // Step 2: No cache, fetch from network
        if (navigator.onLine && !cancelled) {
          try {
            const result = await api.getConversationBySlug(slug);
            if (!cancelled) {
              setConversation(result.conversation);
              setMessages(result.messages);
              setAgentWorking(result.agent_working);
              setContextWindowUsed(result.context_window_size || 0);
              setConversationId(result.conversation.id);
              
              // Cache it
              await cacheDB.putConversation(result.conversation);
              await cacheDB.putMessages(result.messages);
            }
          } catch (err) {
            if (!cancelled) {
              setError(err instanceof Error ? err.message : 'Failed to load conversation');
            }
          }
        } else if (!cancelled) {
          setError('Conversation not found in cache and offline');
        }
      } catch (err) {
        if (!cancelled) {
          console.error('Failed to load conversation:', err);
          setError(err instanceof Error ? err.message : 'Failed to load conversation');
        }
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

  // Send a message
  const sendMessage = useCallback(async (localId: string, text: string, images: { data: string; media_type: string }[] = []) => {
    if (!conversationId) return;

    sendingMessagesRef.current.add(localId);

    try {
      if (isOnline) {
        await api.sendMessage(conversationId, text, images, localId);
        markSentRef.current(localId);
        setAgentWorking(true);
        setBreadcrumbs([{ type: 'user', label: 'User' }]);
      } else {
        await queueOperation({
          type: 'send_message',
          conversationId,
          payload: { text, images, localId },
          createdAt: new Date(),
          retryCount: 0,
          status: 'pending'
        });
        markSentRef.current(localId);
        setAgentWorking(false);
      }
    } catch (err) {
      console.error('Failed to send message:', err);
      markFailedRef.current(localId);
    } finally {
      sendingMessagesRef.current.delete(localId);
    }
  }, [conversationId, isOnline, queueOperation]);

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
  }, [isConnected, conversationId, queuedMessages]);

  const handleSend = (text: string, attachedImages: ImageData[]) => {
    if (!conversationId) return;

    clearDraft();
    setImages([]);

    const msg = enqueue(text, attachedImages);

    if (isConnected) {
      sendMessage(msg.localId, text, attachedImages);
    }
  };

  const handleRetry = (localId: string) => {
    const msg = queuedMessages.find(m => m.localId === localId);
    if (!msg) return;

    retry(localId);
    
    if (isConnected) {
      sendMessage(localId, msg.text, msg.images);
    }
  };

  const handleCancel = async () => {
    if (!conversationId || !agentWorking) return;
    if (convState.startsWith('cancelling')) return;

    try {
      await api.cancelConversation(conversationId);
    } catch (err) {
      console.error('Failed to cancel:', err);
    }
  };

  // Manual continuation trigger (REQ-BED-023)
  const handleTriggerContinuation = async () => {
    if (!conversationId || convState !== 'idle') return;

    try {
      await api.triggerContinuation(conversationId);
    } catch (err) {
      console.error('Failed to trigger continuation:', err);
    }
  };

  const handleOpenFileBrowser = useCallback(() => {
    setShowFileBrowser(true);
  }, []);

  const handleFileSelect = useCallback((filePath: string, rootDir: string) => {
    setShowFileBrowser(false);
    setProseReaderFile({ path: filePath, rootDir });
  }, []);

  const handleCloseProseReader = useCallback(() => {
    setProseReaderFile(null);
  }, []);

  const handleSendNotes = useCallback((formattedNotes: string) => {
    if (draft.trim()) {
      setDraft(draft + '\n\n' + formattedNotes);
    } else {
      setDraft(formattedNotes);
    }
    setProseReaderFile(null);
  }, [draft, setDraft]);

  const handleOpenFileFromPatch = useCallback((filePath: string, modifiedLines: Set<number>, firstModifiedLine: number) => {
    const rootDir = conversation?.cwd || '/';
    const fullPath = filePath.startsWith('/') ? filePath : `${rootDir}/${filePath}`;
    setProseReaderFile({
      path: fullPath,
      rootDir,
      patchContext: { modifiedLines, firstModifiedLine },
    });
  }, [conversation?.cwd]);

  if (error) {
    return (
      <div id="app">
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
        <main id="main-area">
          <section id="chat-view" className="view active">
            <div id="messages">
              <MessageListSkeleton count={4} />
            </div>
          </section>
        </main>
      </div>
    );
  }

  const canSend = !agentWorking;
  const isCancelling = convState.startsWith('cancelling');

  return (
    <div id="app">
      <MessageList
        messages={messages}
        queuedMessages={queuedMessages}
        convState={convState}
        stateData={stateData}
        onRetry={handleRetry}
        onOpenFile={handleOpenFileFromPatch}
      />
      {convState === 'error' && stateData?.message && (
        <ErrorBanner
          message={stateData.message}
          onRetry={() => handleSend('continue', [])}
        />
      )}
      {convState === 'context_exhausted' && contextExhaustedSummary && (
        <div className="context-exhausted-banner">
          <div className="context-exhausted-header">
            <span className="context-exhausted-icon">⚠️</span>
            <span className="context-exhausted-title">Context Window Full</span>
          </div>
          <div className="context-exhausted-summary">
            <p>This conversation has reached its context limit. Copy the summary below to continue in a new conversation:</p>
            <pre className="context-exhausted-content">{contextExhaustedSummary}</pre>
            <button 
              className="context-exhausted-copy"
              onClick={() => {
                navigator.clipboard.writeText(contextExhaustedSummary);
              }}
            >
              Copy Summary
            </button>
          </div>
        </div>
      )}
      <InputArea
        draft={draft}
        setDraft={setDraft}
        images={images}
        setImages={setImages}
        canSend={canSend}
        agentWorking={agentWorking}
        isCancelling={isCancelling}
        isOffline={isOffline}
        queuedMessages={queuedMessages}
        onSend={handleSend}
        onCancel={handleCancel}
        onRetry={handleRetry}
        onOpenFileBrowser={handleOpenFileBrowser}
      />
      <BreadcrumbBar
        breadcrumbs={breadcrumbs}
        visible={breadcrumbs.length > 0}
      />
      <StateBar
        conversation={conversation}
        convState={convState}
        stateData={stateData}
        connectionState={connectionInfo.state}
        connectionAttempt={connectionInfo.attempt}
        nextRetryIn={connectionInfo.nextRetryIn}
        contextWindowUsed={contextWindowUsed}
        modelContextWindow={modelContextWindow}
        onRetryNow={connectionInfo.retryNow}
        onTriggerContinuation={handleTriggerContinuation}
      />

      <FileBrowser
        isOpen={showFileBrowser}
        rootPath={conversation.cwd}
        conversationId={conversation.id}
        onClose={() => setShowFileBrowser(false)}
        onFileSelect={handleFileSelect}
      />

      {proseReaderFile && (
        <ProseReader
          filePath={proseReaderFile.path}
          rootDir={proseReaderFile.rootDir}
          onClose={handleCloseProseReader}
          onSendNotes={handleSendNotes}
          patchContext={proseReaderFile.patchContext ?? undefined}
        />
      )}
    </div>
  );
}
