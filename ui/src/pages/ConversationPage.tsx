import { useState, useEffect, useRef, useCallback } from 'react';
import { useParams, useNavigate } from 'react-router-dom';
import { api, ExpansionError, type Conversation, type ImageData, type ModelInfo } from '../api';
import { isAgentWorking, isCancellingState, parseConversationState } from '../utils';
import { cacheDB } from '../cache';
import { MessageList } from '../components/MessageList';
import { InputArea } from '../components/InputArea';
import type { InputAreaHandle } from '../components/InputArea';
import { MessageListSkeleton } from '../components/Skeleton';
import { FileBrowserOverlay, useFileExplorer } from '../components/FileExplorer';
import { ProseReader } from '../components/ProseReader';
import { TaskApprovalReader } from '../components/TaskApprovalReader';
import { QuestionPanel } from '../components/QuestionPanel';
import { FirstTaskWelcome } from '../components/FirstTaskWelcome';
import { useMessageQueue, useConnection } from '../hooks';
import { useToast } from '../hooks/useToast';
import { Toast } from '../components/Toast';
import { useAppMachine } from '../hooks/useAppMachine';
import { StateBar } from '../components/StateBar';
import { BreadcrumbBar } from '../components/BreadcrumbBar';
import { ErrorBanner } from '../components/ErrorBanner';
import { WorkActions } from '../components/WorkActions';
import { useConversationAtom } from '../conversation';

export function ConversationPage() {
  const { slug } = useParams<{ slug: string }>();
  const navigate = useNavigate();

  // Atom-backed conversation state (survives navigation via ConversationProvider)
  const [atom, dispatch] = useConversationAtom(slug!);

  // Derived from atom
  const conversationId = atom.conversationId ?? undefined;
  const conversation = atom.conversation;

  // Page-level state — not conversation data
  const [error, setError] = useState<string | null>(null);
  const [conversationIdForSSE, setConversationIdForSSE] = useState<string | undefined>(
    undefined
  );

  // File explorer context (shared with desktop panel)
  const fileExplorer = useFileExplorer();
  const [isDesktop] = useState(() => window.matchMedia('(min-width: 1025px)').matches);

  // Mobile-only overlays
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

  // Toast for question panel feedback
  const { toasts, dismissToast, showInfo } = useToast();

  // Image attachments (not conversation state — cleared on page refresh)
  const [images, setImages] = useState<ImageData[]>([]);

  // Available models (for upgrade detection)
  const [availableModels, setAvailableModels] = useState<ModelInfo[]>([]);

  // Task approval overlay
  const [showTaskApproval, setShowTaskApproval] = useState(false);
  const [showFirstTaskWelcome, setShowFirstTaskWelcome] = useState(false);

  // Message queue management
  const { queuedMessages, enqueue, markSent, markFailed, dismiss } =
    useMessageQueue(conversationId);

  // Connection lifecycle — receives dispatch and lastSequenceId from atom
  const connectionInfo = useConnection({
    conversationId: conversationIdForSSE,
    lastSequenceId: atom.lastSequenceId,
    dispatch,
  });

  const isOffline =
    connectionInfo.state === 'offline' || connectionInfo.state === 'reconnecting';
  const isConnected =
    connectionInfo.state === 'connected' || connectionInfo.state === 'reconnected';

  // Ref to read atom state inside effects without adding it to deps
  const atomRef = useRef(atom);
  atomRef.current = atom;

  // Load conversation by slug — skip if atom already has data from a previous visit
  useEffect(() => {
    if (!slug) {
      navigate('/');
      return;
    }

    setError(null);

    // Returning navigation: atom already has conversationId — just reconnect SSE.
    // Reading via ref to avoid adding `atom` to deps (would re-run on every SSE event).
    if (atomRef.current.conversationId) {
      return;
    }

    let cancelled = false;

    const loadConversation = async () => {
      try {
        // Step 1: Show cached data immediately
        const cached = await cacheDB.getConversationBySlug(slug);
        if (cached && !cancelled) {
          const cachedMessages = await cacheDB.getMessages(cached.id);
          dispatch({
            type: 'set_initial_data',
            conversationId: cached.id,
            conversation: cached,
            messages: cachedMessages,
            phase: cached.state ? parseConversationState(cached.state) : { type: 'idle' },
            contextWindow: { used: 0, total: 200_000 },
          });
        }

        // Step 2: Fetch authoritative data from network
        if (navigator.onLine && !cancelled) {
          try {
            const result = await api.getConversationBySlug(slug);
            if (!cancelled) {
              dispatch({
                type: 'set_initial_data',
                conversationId: result.conversation.id,
                conversation: result.conversation,
                messages: result.messages,
                phase: result.conversation.state
                  ? parseConversationState(result.conversation.state)
                  : result.display_state === 'working'
                    ? { type: 'awaiting_llm' }
                    : { type: 'idle' },
                contextWindow: {
                  used: result.context_window_size || 0,
                  total: 200_000,
                },
              });
              await cacheDB.putConversation(result.conversation);
              await cacheDB.putMessages(result.messages);
            }
          } catch (err) {
            if (!cancelled) {
              if (!cached) {
                setError(
                  err instanceof Error ? err.message : 'Failed to load conversation'
                );
              }
            }
          }
        } else if (!cancelled && !cached) {
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
  }, [slug, navigate, dispatch]);

  // Defer SSE connection to not block initial render
  useEffect(() => {
    const convId = atom.conversationId;
    if (!convId) {
      setConversationIdForSSE(undefined);
      return;
    }

    const timer = setTimeout(() => {
      setConversationIdForSSE(convId);
    }, 100);

    return () => {
      clearTimeout(timer);
      setConversationIdForSSE(undefined);
    };
  }, [atom.conversationId]);

  // Fetch system prompt once when conversationId is known
  useEffect(() => {
    if (!conversationId) return;
    api
      .getSystemPrompt(conversationId)
      .then((sp) => dispatch({ type: 'set_system_prompt', systemPrompt: sp }))
      .catch((err) => console.warn('Failed to load system prompt:', err));
  }, [conversationId, dispatch]);

  // Fetch available models once (for upgrade detection in StateBar)
  useEffect(() => {
    api.listModels()
      .then((resp) => setAvailableModels(resp.models))
      .catch((err) => console.warn('Failed to load models:', err));
  }, []);

  // Auto-open/close task approval overlay on state transitions
  useEffect(() => {
    if (atom.phase.type === 'awaiting_task_approval') {
      setShowTaskApproval(true);
    } else {
      setShowTaskApproval(false);
    }
  }, [atom.phase.type]);

  // Cache new messages as they arrive via SSE
  const cachedMsgCountRef = useRef(0);
  useEffect(() => {
    const msgs = atom.messages;
    if (msgs.length > cachedMsgCountRef.current) {
      const newMsgs = msgs.slice(cachedMsgCountRef.current);
      cachedMsgCountRef.current = msgs.length;
      void cacheDB.putMessages(newMsgs);
    }
  }, [atom.messages]);

  // Cache conversation metadata when it changes
  useEffect(() => {
    if (atom.conversation) {
      void cacheDB.putConversation(atom.conversation);
    }
  }, [atom.conversation]);

  // Stable refs for queue callbacks
  const markSentRef = useRef(markSent);
  const markFailedRef = useRef(markFailed);
  useEffect(() => { markSentRef.current = markSent; }, [markSent]);
  useEffect(() => { markFailedRef.current = markFailed; }, [markFailed]);

  const sendMessage = useCallback(
    async (
      localId: string,
      text: string,
      imgs: { data: string; media_type: string }[] = []
    ) => {
      if (!conversationId) return;

      sendingMessagesRef.current.add(localId);

      try {
        if (isOnline) {
          await api.sendMessage(conversationId, text, imgs, localId);
          markSentRef.current(localId);
          dispatch({ type: 'sse_state_change', phase: { type: 'awaiting_llm' } });
          dispatch({
            type: 'sse_message',
            message: {
              message_id: localId,
              sequence_id: -1, // Optimistic — will be replaced by SSE
              conversation_id: conversationId,
              message_type: 'user',
              content: { text },
              created_at: new Date().toISOString(),
            },
            sequenceId: -1,
          });
        } else {
          await queueOperation({
            type: 'send_message',
            conversationId,
            payload: { text, images: imgs, localId },
            createdAt: new Date(),
            retryCount: 0,
            status: 'pending',
          });
          markSentRef.current(localId);
        }
      } catch (err) {
        if (err instanceof ExpansionError) {
          // Don't mark as failed — the user needs to fix the reference.
          // Remove from queue so it doesn't show as a failed message.
          markFailedRef.current(localId);
          // Re-throw so InputArea can display inline error (REQ-IR-007)
          throw err;
        }
        console.error('Failed to send message:', err);
        markFailedRef.current(localId);
      } finally {
        sendingMessagesRef.current.delete(localId);
      }
    },
    [conversationId, isOnline, queueOperation, dispatch]
  );

  const sendMessageRef = useRef(sendMessage);
  useEffect(() => { sendMessageRef.current = sendMessage; }, [sendMessage]);

  // Send queued messages when connection is restored
  useEffect(() => {
    if (!isConnected || !conversationId) return;

    const pending = queuedMessages.filter(
      (m) => m.status === 'sending' && !sendingMessagesRef.current.has(m.localId)
    );

    for (const msg of pending) {
      sendMessageRef.current(msg.localId, msg.text, msg.images);
    }
  }, [isConnected, conversationId, queuedMessages]);

  const handleSend = async (text: string, attachedImages: ImageData[]) => {
    if (!conversationId) return;

    const msg = enqueue(text, attachedImages);

    if (isConnected) {
      // Await so expansion errors propagate back to InputArea (REQ-IR-007)
      await sendMessage(msg.localId, text, attachedImages);
    }
  };

  const handleRetry = (localId: string) => {
    const msg = queuedMessages.find((m) => m.localId === localId);
    if (!msg) return;

    // Populate the message back into the input area for review/editing
    // instead of directly resending (the banner truncates content and
    // the user may want to fix the issue that caused the failure).
    dismiss(localId);
    inputRef.current?.setDraft(msg.text);
  };

  const handleCancel = async () => {
    if (!conversationId || !isAgentWorking(atom.phase)) return;
    if (isCancellingState(atom.phase)) return;

    try {
      await api.cancelConversation(conversationId);
    } catch (err) {
      console.error('Failed to cancel:', err);
    }
  };

  const handleTriggerContinuation = async () => {
    if (!conversationId || atom.phase.type !== 'idle') return;

    try {
      await api.triggerContinuation(conversationId);
    } catch (err) {
      console.error('Failed to trigger continuation:', err);
    }
  };

  const handleUpgradeModel = async (newModelId: string) => {
    if (!conversationId || atom.phase.type !== 'idle') return;

    try {
      await api.upgradeModel(conversationId, newModelId);
      // Backend evicts the runtime on upgrade; reload to reconnect with new model
      window.location.reload();
    } catch (err) {
      console.error('Failed to upgrade model:', err);
    }
  };

  const handleApproveTask = async () => {
    if (!conversationId) return;
    try {
      const result = await api.approveTask(conversationId);
      if (result.first_task) {
        setShowFirstTaskWelcome(true);
      }
    } catch (err) {
      console.error('Failed to approve task:', err);
    }
  };

  const handleRejectTask = async () => {
    if (!conversationId) return;
    try {
      await api.rejectTask(conversationId);
    } catch (err) {
      console.error('Failed to reject task:', err);
    }
  };

  const handleTaskFeedback = async (annotations: string) => {
    if (!conversationId) return;
    try {
      await api.sendTaskFeedback(conversationId, annotations);
    } catch (err) {
      console.error('Failed to send task feedback:', err);
    }
  };

  // File browser opened from sidebar on desktop; mobile overlay triggered elsewhere

  const handleFileSelect = useCallback(
    (filePath: string, rootDir: string) => {
      setShowFileBrowser(false);
      if (isDesktop) {
        fileExplorer.openFile(filePath, rootDir);
      } else {
        setMobileProseFile({ path: filePath, rootDir });
      }
    },
    [isDesktop, fileExplorer]
  );

  const handleCloseProseReader = useCallback(() => {
    if (isDesktop) {
      fileExplorer.closeFile();
    } else {
      setMobileProseFile(null);
    }
  }, [isDesktop, fileExplorer]);

  const handleSendNotes = useCallback(
    (formattedNotes: string) => {
      if (inputRef.current) {
        // InputArea is mounted (mobile path) — update via React state.
        inputRef.current.appendToDraft(formattedNotes);
      } else if (conversationId) {
        // Desktop early-return renders ProseReader instead of InputArea,
        // so inputRef.current is null. Write directly to localStorage so
        // InputArea picks it up when it mounts after the prose reader closes.
        const key = `phoenix:draft:${conversationId}`;
        try {
          const existing = localStorage.getItem(key) ?? '';
          const next = existing.trim() ? existing + '\n\n' + formattedNotes : formattedNotes;
          localStorage.setItem(key, next);
        } catch (e) {
          console.warn('Failed to save notes to draft:', e);
        }
      }
      if (isDesktop) {
        fileExplorer.closeFile();
      } else {
        setMobileProseFile(null);
      }
    },
    [isDesktop, fileExplorer, conversationId]
  );

  const handleOpenFileFromPatch = useCallback(
    (filePath: string, modifiedLines: Set<number>, firstModifiedLine: number) => {
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
    },
    [conversation?.cwd, isDesktop, fileExplorer]
  );

  if (error) {
    return (
      <div id="app">
        <main id="main-area">
          <div className="empty-state">
            <div className="empty-state-icon">❌</div>
            <p>{error}</p>
            <button
              className="btn-primary"
              onClick={() => navigate('/')}
              style={{ marginTop: 16 }}
            >
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

  const convStateForChildren = atom.phase;

  return (
    <div id="app">
      <MessageList
        messages={atom.messages}
        queuedMessages={queuedMessages}
        convState={convStateForChildren}
        onRetry={handleRetry}
        onOpenFile={handleOpenFileFromPatch}
        conversationId={conversationId}
        streamingBuffer={atom.streamingBuffer}
        {...(atom.systemPrompt !== undefined && atom.systemPrompt !== null && {
          systemPrompt: atom.systemPrompt,
        })}
      />
      {atom.uiError && (
        <div className="sse-error-toast" role="alert">
          <span className="sse-error-text">
            {atom.uiError.type === 'BackendError' ? atom.uiError.message : 'Connection error'}
          </span>
          <button className="sse-error-dismiss" onClick={() => dispatch({ type: 'clear_error' })}>
            Dismiss
          </button>
        </div>
      )}
      {convStateForChildren.type === 'context_exhausted' && (
        <div className="context-exhausted-banner">
          <div className="context-exhausted-header">
            <span className="context-exhausted-icon">⚠️</span>
            <span className="context-exhausted-title">Context Window Full</span>
          </div>
          <div className="context-exhausted-summary">
            <p>
              This conversation has reached its context limit. Copy the summary below
              to continue in a new conversation:
            </p>
            <pre className="context-exhausted-content">
              {convStateForChildren.summary}
            </pre>
            <button
              className="context-exhausted-copy"
              onClick={() => {
                navigator.clipboard.writeText(
                  convStateForChildren.type === 'context_exhausted'
                    ? convStateForChildren.summary
                    : ''
                );
              }}
            >
              Copy Summary
            </button>
          </div>
        </div>
      )}
      {convStateForChildren.type === 'terminal' && (
        <div className="terminal-banner">
          <button
            className="btn-primary"
            onClick={() => navigate('/new')}
          >
            Start new conversation
          </button>
        </div>
      )}
      {convStateForChildren.type === 'error' ? (
        <ErrorBanner
          message={convStateForChildren.message}
          onRetry={() => handleSend('continue', [])}
          onDismiss={() => dispatch({ type: 'sse_state_change', phase: { type: 'idle' } })}
        />
      ) : convStateForChildren.type === 'awaiting_user_response' ? (
        <QuestionPanel
          questions={convStateForChildren.questions}
          conversationId={conversation.id}
          showToast={showInfo}
        />
      ) : convStateForChildren.type !== 'context_exhausted' && convStateForChildren.type !== 'awaiting_task_approval' && convStateForChildren.type !== 'terminal' ? (
        <>
        {conversationId && (
          <WorkActions
            conversationId={conversationId}
            convModeLabel={conversation.conv_mode_label}
            phaseType={convStateForChildren.type}
            branchName={conversation.branch_name ?? undefined}
            baseBranch={conversation.base_branch}
            onSendMessage={(text) => handleSend(text, [])}
          />
        )}
        <InputArea
          ref={inputRef}
          conversationId={conversationId}
          convState={convStateForChildren}
          images={images}
          setImages={setImages}
          isOffline={isOffline}
          queuedMessages={queuedMessages}
          convModeLabel={conversation.conv_mode_label}
          onSend={handleSend}
          onCancel={handleCancel}
          onRetry={handleRetry}
          onDismissError={dismiss}
        />
        </>
      ) : null}
      <BreadcrumbBar breadcrumbs={atom.breadcrumbs} visible={atom.breadcrumbs.length > 0} />
      <StateBar
        conversation={conversation as Conversation}
        convState={convStateForChildren}
        connectionState={connectionInfo.state}
        connectionAttempt={connectionInfo.attempt}
        nextRetryIn={connectionInfo.nextRetryIn}
        contextWindowUsed={atom.contextWindow.used}
        modelContextWindow={atom.contextWindow.total}
        availableModels={availableModels}
        onRetryNow={connectionInfo.retryNow}
        onTriggerContinuation={handleTriggerContinuation}
        onUpgradeModel={handleUpgradeModel}
      />

      {/* Task approval overlay — browser back navigates away; SSE restores state on return. */}
      {showTaskApproval && atom.phase.type === 'awaiting_task_approval' && (
        <TaskApprovalReader
          title={atom.phase.title}
          priority={atom.phase.priority}
          plan={atom.phase.plan}
          onApprove={handleApproveTask}
          onReject={handleRejectTask}
          onSendFeedback={handleTaskFeedback}
        />
      )}

      <Toast messages={toasts} onDismiss={dismissToast} />

      {/* First task welcome modal */}
      <FirstTaskWelcome
        visible={showFirstTaskWelcome}
        onClose={() => setShowFirstTaskWelcome(false)}
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
