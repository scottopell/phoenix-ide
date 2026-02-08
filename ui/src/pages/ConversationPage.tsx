import { useState, useEffect, useRef, useCallback } from 'react';
import { useParams, useNavigate } from 'react-router-dom';
import { enhancedApi } from '../enhancedApi';
import { Conversation, Message, ConversationState, SseEventType, SseEventData, SseInitData, SseMessageData, SseStateChangeData, ImageData } from '../api';
// Header removed - navigation and status moved to input area
import { MessageList } from '../components/MessageList';
import { VirtualizedMessageList } from '../components/VirtualizedMessageList';
import { InputArea } from '../components/InputArea';
import { MessageListSkeleton } from '../components/Skeleton';
import { FileBrowser } from '../components/FileBrowser';
import { ProseReader } from '../components/ProseReader';
import { useDraft, useMessageQueue, useConnection } from '../hooks';
import { useAppMachine } from '../hooks/useAppMachine';
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
  const [_breadcrumbs, setBreadcrumbs] = useState<Breadcrumb[]>([]);
  const [agentWorking, setAgentWorking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [_contextWindowUsed, setContextWindowUsed] = useState(0);
  const [initialLoadComplete, setInitialLoadComplete] = useState(false);
  const [lastDataSource, setLastDataSource] = useState<'memory' | 'indexeddb' | 'network' | null>(null);
  
  // File browser and prose reader state (REQ-PF-001 through REQ-PF-014)
  const [showFileBrowser, setShowFileBrowser] = useState(false);
  const [proseReaderFile, setProseReaderFile] = useState<{
    path: string;
    rootDir: string;
    patchContext?: {
      modifiedLines: Set<number>;
      firstModifiedLine?: number;
    };
  } | null>(null);

  const sendingMessagesRef = useRef<Set<string>>(new Set()); // Track localIds being sent

  // App state for offline support
  const { isOnline, queueOperation, showSyncStatus: _showSyncStatus } = useAppMachine();

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
          
          // On reconnection, we request ?after=lastSeqId and get only NEW messages.
          // If we already have messages, append new ones; otherwise replace.
          const newMessages = initData.messages || [];
          setMessages((prev) => {
            if (prev.length === 0) {
              // First connection - use server's full list
              return newMessages;
            }
            // Reconnection - append new messages, deduplicating by sequence_id
            const existingIds = new Set(prev.map(m => m.sequence_id));
            const toAdd = newMessages.filter(m => !existingIds.has(m.sequence_id));
            return toAdd.length > 0 ? [...prev, ...toAdd] : prev;
          });
          
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
    conversationId: conversationIdForSSE,
    onEvent: handleSseEvent,
  });
  
  // Defer SSE connection to not block initial render
  useEffect(() => {
    if (!conversationId) return;
    
    console.log('[ConversationPage] Deferring SSE connection...');
    const timer = setTimeout(() => {
      console.log('[ConversationPage] Starting SSE connection');
      setConversationIdForSSE(conversationId);
    }, 100); // Small delay to let UI render first
    
    return () => {
      clearTimeout(timer);
      setConversationIdForSSE(undefined);
    };
  }, [conversationId]);

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
      console.log(`[ConversationPage] Starting to load conversation: ${slug}`);
      const loadStart = Date.now();
      
      try {
        // First, try to load from cache for instant display
        const cachedResult = await enhancedApi.getConversationBySlug(slug, { forceFresh: false });
        const loadDuration = Date.now() - loadStart;
        console.log(`[ConversationPage] Load completed in ${loadDuration}ms from ${cachedResult.source}`);
        
        if (!cancelled && cachedResult.data) {
          setConversation(cachedResult.data.conversation);
          setMessages(cachedResult.data.messages);
          setAgentWorking(cachedResult.data.agent_working);
          setContextWindowUsed(cachedResult.data.context_window_size || 0);
          setLastDataSource(cachedResult.source);
          setInitialLoadComplete(true);
          
          // Set conversation ID for hooks - this triggers the SSE connection
          setConversationId(cachedResult.data.conversation.id);
          
          // If data was stale, it will auto-refresh in background
          if (cachedResult.stale) {
            console.log(`Loaded stale ${cachedResult.source} data, background refresh triggered`);
          }
        }
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
      if (isOnline) {
        await enhancedApi.sendMessage(conversationId, text, images, localId);
        markSentRef.current(localId);
        setAgentWorking(true);
        setBreadcrumbs([{ type: 'user', label: 'User' }]);
      } else {
        // Queue for offline sync
        await queueOperation({
          type: 'send_message',
          conversationId,
          payload: { text, images, localId },
          createdAt: new Date(),
          retryCount: 0,
          status: 'pending'
        });
        markSentRef.current(localId);
        // Show offline indicator instead of agent working
        setAgentWorking(false);
      }
    } catch (err) {
      console.error('Failed to send message:', err);
      markFailedRef.current(localId);
    } finally {
      sendingMessagesRef.current.delete(localId);
    }
  }, [conversationId, isOnline, queueOperation]); // Added isOnline and queueOperation

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
  const handleSend = (text: string, attachedImages: ImageData[]) => {
    if (!conversationId) return;

    // Clear draft and images
    clearDraft();
    setImages([]);

    // Enqueue the message (shows immediately with sending state)
    const msg = enqueue(text, attachedImages);

    // If we're connected, send immediately
    if (isConnected) {
      sendMessage(msg.localId, text, attachedImages);
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
      await enhancedApi.cancelConversation(conversationId);
    } catch (err) {
      console.error('Failed to cancel:', err);
    }
  };

  // File browser handlers (REQ-PF-001 through REQ-PF-004)
  const handleOpenFileBrowser = useCallback(() => {
    setShowFileBrowser(true);
  }, []);

  const handleFileSelect = useCallback((filePath: string, rootDir: string) => {
    setShowFileBrowser(false);
    setProseReaderFile({ path: filePath, rootDir });
  }, []);

  // Prose reader handlers (REQ-PF-005 through REQ-PF-014)
  const handleCloseProseReader = useCallback(() => {
    setProseReaderFile(null);
  }, []);

  const handleSendNotes = useCallback((formattedNotes: string) => {
    // Inject notes into the draft (REQ-PF-009)
    if (draft.trim()) {
      setDraft(draft + '\n\n' + formattedNotes);
    } else {
      setDraft(formattedNotes);
    }
    setProseReaderFile(null);
  }, [draft, setDraft]);

  // Open file from patch output (REQ-PF-014)
  const handleOpenFileFromPatch = useCallback((filePath: string, modifiedLines: Set<number>, firstModifiedLine: number) => {
    // Resolve the file path against the conversation's cwd
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
      {/* Data source indicator for debugging - only in development */}
      {import.meta.env.DEV && lastDataSource && initialLoadComplete && (
        <div className="data-source-indicator">
          Loaded from: {lastDataSource}
        </div>
      )}
      {/* Use virtualized list for large conversations */}
      {messages.length > 50 ? (
        <VirtualizedMessageList
          messages={messages}
          queuedMessages={queuedMessages}
          convState={convState}
          stateData={stateData}
          onRetry={handleRetry}
          onOpenFile={handleOpenFileFromPatch}
        />
      ) : (
        <MessageList
          messages={messages}
          queuedMessages={queuedMessages}
          convState={convState}
          stateData={stateData}
          onRetry={handleRetry}
          onOpenFile={handleOpenFileFromPatch}
        />
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
        conversationSlug={conversation.slug}
        convState={convState}
        stateData={stateData}
      />

      {/* File Browser (REQ-PF-001 through REQ-PF-004) */}
      <FileBrowser
        isOpen={showFileBrowser}
        rootPath={conversation.cwd}
        conversationId={conversation.id}
        onClose={() => setShowFileBrowser(false)}
        onFileSelect={handleFileSelect}
      />

      {/* Prose Reader (REQ-PF-005 through REQ-PF-013) */}
      {proseReaderFile && (
        <ProseReader
          filePath={proseReaderFile.path}
          rootDir={proseReaderFile.rootDir}
          onClose={handleCloseProseReader}
          onSendNotes={handleSendNotes}
          patchContext={proseReaderFile.patchContext}
        />
      )}
    </div>
  );
}
