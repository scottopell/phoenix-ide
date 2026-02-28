import { useState, useEffect, useRef, useCallback } from 'react';
import { useParams, useNavigate } from 'react-router-dom';
import { api, Conversation, Message, ConversationState, SseEventType, SseEventData, SseInitData, SseMessageData, SseStateChangeData, ImageData } from '../api';
import { isAgentWorking, isCancellingState, parseConversationState } from '../utils';
import { cacheDB } from '../cache';
import { MessageList } from '../components/MessageList';
import { InputArea } from '../components/InputArea';
import type { InputAreaHandle } from '../components/InputArea';
import { MessageListSkeleton } from '../components/Skeleton';
import { FileBrowserOverlay, useFileExplorer } from '../components/FileExplorer';
import { ProseReader } from '../components/ProseReader';
import { useMessageQueue, useConnection } from '../hooks';
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
  const [convState, setConvState] = useState<ConversationState>({ type: 'idle' });
  const [breadcrumbs, setBreadcrumbs] = useState<Breadcrumb[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [contextWindowUsed, setContextWindowUsed] = useState(0);
  const [modelContextWindow, setModelContextWindow] = useState(200_000); // Default fallback
  const [systemPrompt, setSystemPrompt] = useState<string | undefined>(undefined);

  
  // File explorer context (shared with desktop panel)
  const fileExplorer = useFileExplorer();
  const [isDesktop] = useState(() => window.matchMedia('(min-width: 1025px)').matches);

  // Mobile-only local state for file browser and prose reader overlays
  const [showFileBrowser, setShowFileBrowser] = useState(false);
  const [mobileProseFile, setMobileProseFile] = useState<{
    path: string;
    rootDir: string;
    patchContext?: {
      modifiedLines: Set<number>;
      firstModifiedLine?: number;
    };
  } | null>(null);

  const sendingMessagesRef = useRef<Set<string>>(new Set());
  const inputRef = useRef<InputAreaHandle>(null);

  // App state for offline support
  const { isOnline, queueOperation } = useAppMachine();

  // Image attachments (not persisted - cleared on page refresh)
  const [images, setImages] = useState<ImageData[]>([]);

  // Message queue management
  const { queuedMessages, enqueue, markSent, markFailed, retry } = useMessageQueue(conversationId);

  // Update breadcrumbs from state
  const updateBreadcrumbsFromState = useCallback((state: ConversationState) => {
    switch (state.type) {
      case 'idle': case 'error': case 'terminal': case 'context_exhausted':
      case 'awaiting_llm': case 'awaiting_continuation':
      case 'cancelling': case 'cancelling_tool': case 'cancelling_sub_agents':
        return;

      case 'llm_requesting': {
        const attempt = state.attempt;
        const label = attempt > 1 ? `LLM (retry ${attempt})` : 'LLM';
        setBreadcrumbs((prev) => {
          const filtered = prev.filter((b) => b.type !== 'llm');
          filtered.push({ type: 'llm', label });
          return filtered;
        });
        return;
      }

      case 'tool_executing': {
        const toolName = state.current_tool.input?._tool || 'tool';
        const toolId = state.current_tool.id;
        const remaining = state.remaining_tools.length;
        const label = remaining > 0 ? `${String(toolName)} (+${remaining})` : String(toolName);
        setBreadcrumbs((prev) => {
          if (prev.some((b) => b.type === 'tool' && b.toolId === toolId)) return prev;
          return [...prev, { type: 'tool', label, toolId }];
        });
        return;
      }

      case 'awaiting_sub_agents': {
        const pending = state.pending.length;
        const completed = state.completed_results.length;
        const total = pending + completed;
        const label = `sub-agents (${completed}/${total})`;
        setBreadcrumbs((prev) => {
          const updated = [...prev];
          const existing = updated.find((b) => b.type === 'subagents');
          if (existing) {
            existing.label = label;
            return updated;
          }
          return [...prev, { type: 'subagents', label }];
        });
        return;
      }

      default: state satisfies never;
    }
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

          const initState = parseConversationState(initData.conversation?.state);
          setConvState(initState);

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
          updateBreadcrumbsFromState(initState);

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
          const newState = parseConversationState(stateChangeData.state);
          setConvState(newState);
          updateBreadcrumbsFromState(newState);
          break;
        }

        case 'agent_done':
          setConvState({ type: 'idle' });
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
        // Step 1: Show cached data immediately while network fetch is in-flight
        // (stale-while-revalidate — sidebar polling populates conversation records
        // but never messages, so cache-only would silently show empty threads)
        const cached = await cacheDB.getConversationBySlug(slug);
        if (cached && !cancelled) {
          setConversation(cached);
          const cachedMessages = await cacheDB.getMessages(cached.id);
          setMessages(cachedMessages);
          // Do not start SSE yet — wait for network to give us the authoritative
          // sequence ID so SSE resumes from the right position.
        }

        // Step 2: Always fetch fresh data from network
        if (navigator.onLine && !cancelled) {
          try {
            const result = await api.getConversationBySlug(slug);
            if (!cancelled) {
              setConversation(result.conversation);
              setMessages(result.messages);
              setConvState(result.display_state === 'working' ? { type: 'awaiting_llm' } : { type: 'idle' });
              setContextWindowUsed(result.context_window_size || 0);
              setConversationId(result.conversation.id); // Triggers SSE
              await cacheDB.putConversation(result.conversation);
              await cacheDB.putMessages(result.messages);
            }
          } catch (err) {
            if (!cancelled) {
              if (cached) {
                // Network failed but we have cached data — start SSE with that
                setConversationId(cached.id);
              } else {
                setError(err instanceof Error ? err.message : 'Failed to load conversation');
              }
            }
          }
        } else if (!cancelled) {
          if (cached) {
            setConversationId(cached.id); // Offline, use cache and start SSE
          } else {
            setError('Conversation not found in cache and offline');
          }
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

  // Fetch system prompt once when conversation is loaded
  useEffect(() => {
    if (!conversationId) return;
    api.getSystemPrompt(conversationId)
      .then(setSystemPrompt)
      .catch((err) => console.warn('Failed to load system prompt:', err));
  }, [conversationId]);

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
        setConvState({ type: 'awaiting_llm' });
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
    if (!conversationId || !isAgentWorking(convState)) return;
    if (isCancellingState(convState)) return;

    try {
      await api.cancelConversation(conversationId);
    } catch (err) {
      console.error('Failed to cancel:', err);
    }
  };

  // Manual continuation trigger (REQ-BED-023)
  const handleTriggerContinuation = async () => {
    if (!conversationId || convState.type !== 'idle') return;

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
    if (isDesktop) {
      fileExplorer.openFile(filePath, rootDir);
    } else {
      setMobileProseFile({ path: filePath, rootDir });
    }
  }, [isDesktop, fileExplorer]);

  const handleCloseProseReader = useCallback(() => {
    if (isDesktop) {
      fileExplorer.closeFile();
    } else {
      setMobileProseFile(null);
    }
  }, [isDesktop, fileExplorer]);

  const handleSendNotes = useCallback((formattedNotes: string) => {
    inputRef.current?.appendToDraft(formattedNotes);
    if (isDesktop) {
      fileExplorer.closeFile();
    } else {
      setMobileProseFile(null);
    }
  }, [isDesktop, fileExplorer]);

  const handleOpenFileFromPatch = useCallback((filePath: string, modifiedLines: Set<number>, firstModifiedLine: number) => {
    const rootDir = conversation?.cwd || '/';
    const fullPath = filePath.startsWith('/') ? filePath : `${rootDir}/${filePath}`;
    if (isDesktop) {
      fileExplorer.openFile(fullPath, rootDir, { modifiedLines, firstModifiedLine });
    } else {
      setMobileProseFile({
        path: fullPath,
        rootDir,
        patchContext: { modifiedLines, firstModifiedLine },
      });
    }
  }, [conversation?.cwd, isDesktop, fileExplorer]);

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

  // Desktop: prose reader replaces conversation content
  if (isDesktop && fileExplorer.proseReaderState) {
    const prs = fileExplorer.proseReaderState;
    return (
      <div id="app">
        <ProseReader
          filePath={prs.path}
          rootDir={prs.rootDir}
          onClose={handleCloseProseReader}
          onSendNotes={handleSendNotes}
          patchContext={prs.patchContext ?? undefined}
          inline
        />
      </div>
    );
  }

  return (
    <div id="app">
      <MessageList
        messages={messages}
        queuedMessages={queuedMessages}
        convState={convState}
        onRetry={handleRetry}
        onOpenFile={handleOpenFileFromPatch}
        conversationId={conversationId}
        {...(systemPrompt !== undefined && { systemPrompt })}
      />
      {convState.type === 'context_exhausted' && (
        <div className="context-exhausted-banner">
          <div className="context-exhausted-header">
            <span className="context-exhausted-icon">⚠️</span>
            <span className="context-exhausted-title">Context Window Full</span>
          </div>
          <div className="context-exhausted-summary">
            <p>This conversation has reached its context limit. Copy the summary below to continue in a new conversation:</p>
            <pre className="context-exhausted-content">{convState.summary}</pre>
            <button
              className="context-exhausted-copy"
              onClick={() => {
                navigator.clipboard.writeText(convState.type === 'context_exhausted' ? convState.summary : '');
              }}
            >
              Copy Summary
            </button>
          </div>
        </div>
      )}
      {convState.type === 'error' ? (
        <ErrorBanner
          message={convState.message}
          onRetry={() => handleSend('continue', [])}
          onDismiss={() => setConvState({ type: 'idle' })}
        />
      ) : convState.type !== 'context_exhausted' ? (
      <InputArea
        ref={inputRef}
        conversationId={conversationId}
        convState={convState}
        images={images}
        setImages={setImages}
        isOffline={isOffline}
        queuedMessages={queuedMessages}
        onSend={handleSend}
        onCancel={handleCancel}
        onRetry={handleRetry}
        onOpenFileBrowser={handleOpenFileBrowser}
      />
      ) : null}
      <BreadcrumbBar
        breadcrumbs={breadcrumbs}
        visible={breadcrumbs.length > 0}
      />
      <StateBar
        conversation={conversation}
        convState={convState}
        connectionState={connectionInfo.state}
        connectionAttempt={connectionInfo.attempt}
        nextRetryIn={connectionInfo.nextRetryIn}
        contextWindowUsed={contextWindowUsed}
        modelContextWindow={modelContextWindow}
        onRetryNow={connectionInfo.retryNow}
        onTriggerContinuation={handleTriggerContinuation}
      />

      {/* Mobile file browser overlay */}
      <FileBrowserOverlay
        isOpen={showFileBrowser}
        rootPath={conversation.cwd}
        conversationId={conversation.id}
        onClose={() => setShowFileBrowser(false)}
        onFileSelect={handleFileSelect}
      />

      {/* Mobile prose reader overlay */}
      {!isDesktop && mobileProseFile && (
        <ProseReader
          filePath={mobileProseFile.path}
          rootDir={mobileProseFile.rootDir}
          onClose={handleCloseProseReader}
          onSendNotes={handleSendNotes}
          patchContext={mobileProseFile.patchContext ?? undefined}
        />
      )}
    </div>
  );
}
