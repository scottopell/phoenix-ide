import { lazy, Suspense, useState, useEffect, useRef, useCallback, useMemo, type MouseEvent as ReactMouseEvent } from 'react';
import { useParams, useNavigate } from 'react-router-dom';
import { api, ExpansionError, type Conversation, type ImageData } from '../api';
import { refreshModels } from '../modelsPoller';
import { isAgentWorking, isCancellingState, parseConversationState } from '../utils';
import { copyToClipboard } from '../utils/clipboard';
import { cacheDB } from '../cache';
import { MessageList } from '../components/MessageList';
import { InputArea } from '../components/InputArea';
import type { InputAreaHandle } from '../components/InputArea';
import { MessageListSkeleton } from '../components/Skeleton';
import { FileBrowserOverlay, useFileExplorer } from '../components/FileExplorer';
import { PaneDivider } from '../components/PaneDivider';
import { QuestionPanel } from '../components/QuestionPanel';
import {
  useMessageQueue,
  useConnection,
  useModels,
  useAutoAuth,
  derivePendingMessages,
  deriveFailedMessages,
} from '../hooks';
import { useToast } from '../hooks/useToast';
import { Toast } from '../components/Toast';
import { useAppMachine } from '../hooks/useAppMachine';
import { StateBar } from '../components/StateBar';
import { BreadcrumbBar } from '../components/BreadcrumbBar';
import { ErrorBanner } from '../components/ErrorBanner';
import { WorkActions } from '../components/WorkActions';
import { useConversationAtom } from '../conversation';
import { useResizablePane } from '../hooks';

// Conditional overlays / heavy panels — code-split so the default render path
// (chat view with no overlay open) doesn't pay their bundle cost.
// - ProseReader, TaskApprovalReader: pull in react-syntax-highlighter
// - TerminalPanel: pulls in xterm + addon (large)
// - CredentialHelperPanel, FirstTaskWelcome: rarely mounted
const ProseReader = lazy(() =>
  import('../components/ProseReader').then((m) => ({ default: m.ProseReader })),
);
const DiffView = lazy(() =>
  import('../components/viewer/DiffView').then((m) => ({ default: m.DiffView })),
);
const TaskApprovalReader = lazy(() =>
  import('../components/TaskApprovalReader').then((m) => ({ default: m.TaskApprovalReader })),
);
const FirstTaskWelcome = lazy(() =>
  import('../components/FirstTaskWelcome').then((m) => ({ default: m.FirstTaskWelcome })),
);
const CredentialHelperPanel = lazy(() =>
  import('../components/CredentialHelperPanel').then((m) => ({ default: m.CredentialHelperPanel })),
);
const TerminalPanel = lazy(() =>
  import('../components/TerminalPanel').then((m) => ({ default: m.TerminalPanel })),
);

import { ReviewNotesProvider } from '../contexts/ReviewNotesContext';
import {
  DiffViewerStateProvider,
  useDiffViewerState,
} from '../contexts/ViewerStateContext';

const TERMINAL_COLLAPSED_PX = 32;

const AlertTriangle = () => (
  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
    <line x1="12" y1="9" x2="12" y2="13" />
    <line x1="12" y1="17" x2="12.01" y2="17" />
  </svg>
);
const XCircle = () => (
  <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <circle cx="12" cy="12" r="10" />
    <line x1="15" y1="9" x2="9" y2="15" />
    <line x1="9" y1="9" x2="15" y2="15" />
  </svg>
);
const ChevronRightSmall = () => (
  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <polyline points="9 18 15 12 9 6" />
  </svg>
);

export function ConversationPage() {
  return (
    <ReviewNotesProvider>
      <DiffViewerStateProvider>
        <ConversationPageContent />
      </DiffViewerStateProvider>
    </ReviewNotesProvider>
  );
}

function ConversationPageContent() {
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
  // Diff viewer slot — lifted out of WorkActions so the diff can mount
  // inline beside chat at ≥1280px (task 08654 follow-on).
  const diffViewer = useDiffViewerState();
  // Single-slot model: opening one viewer closes the other so the user
  // never sees both fighting for the split pane. When both are set,
  // file wins (most-recent-action — fileExplorer.openFile is what
  // triggered this collision since the user just clicked a file). The
  // alternate ordering (user clicks View Diff while file is open)
  // closes the file via fileExplorer.closeFile in the click handler
  // chain elsewhere; this effect catches the file-clicks-while-diff-open
  // case the click handlers don't reach.
  const closeDiff = diffViewer.close;
  useEffect(() => {
    if (fileExplorer.proseReaderState && diffViewer.payload) {
      closeDiff();
    }
  }, [fileExplorer.proseReaderState, diffViewer.payload, closeDiff]);
  // Close handlers also clear the OTHER viewer to be safe (for cases
  // where state machines briefly hold both during transitions).
  const handleCloseDiff = useCallback(() => closeDiff(), [closeDiff]);
  const [isDesktop] = useState(() => window.matchMedia('(min-width: 1025px)').matches);
  // Wider threshold (≥1280px) gates the split-pane prose reader (task 08654).
  // Below this we keep the existing full-screen overlay UX; above, the
  // reader sits beside the chat as a resizable sibling pane.
  const [isWideDesktop, setIsWideDesktop] = useState(
    () => window.matchMedia('(min-width: 1280px)').matches,
  );
  useEffect(() => {
    const mq = window.matchMedia('(min-width: 1280px)');
    const handler = (e: MediaQueryListEvent) => setIsWideDesktop(e.matches);
    mq.addEventListener('change', handler);
    return () => mq.removeEventListener('change', handler);
  }, []);
  const VIEWER_PANE_MIN = 360;
  const VIEWER_PANE_MAX = 1200;
  const viewerPane = useResizablePane({
    key: 'viewer-pane-width',
    min: VIEWER_PANE_MIN,
    max: VIEWER_PANE_MAX,
    defaultSize: 600,
    collapseThreshold: 280,
  });

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

  // Shared models/credential poller — one request loop app-wide.
  const { models: availableModels, credentialStatus } = useModels();

  // Task approval overlay
  const [showTaskApproval, setShowTaskApproval] = useState(false);
  const [showFirstTaskWelcome, setShowFirstTaskWelcome] = useState(false);
  // Context-full banner: summary expanded by default; user can collapse to
  // read the conversation above.
  const [contextExhaustedExpanded, setContextExhaustedExpanded] = useState(true);
  const [abandoningContextExhausted, setAbandoningContextExhausted] = useState(false);
  // Terminal split-pane height — collapses to a 32px header strip
  const terminalPane = useResizablePane({
    key: 'terminal-height',
    min: TERMINAL_COLLAPSED_PX,
    max: () => Math.min(800, Math.floor(window.innerHeight * 0.75)),
    defaultSize: 300,
    collapseThreshold: 60,
  });

  // Credential helper auto-open — shared hook consolidates the pattern.
  const { showAuthPanel, setShowAuthPanel } = useAutoAuth(credentialStatus);

  // Message queue management. `queuedMessages` is the raw store; the rendered
  // split between "pending in the message list" and "failed in the input area"
  // is derived below.
  const { queuedMessages, enqueue, markFailed, dismiss } =
    useMessageQueue(conversationId);

  // Pending messages shown in the conversation are a pure derivation of the
  // queue and `atom.messages` — see `derivePendingMessages` for the rule.
  const pendingMessages = useMemo(
    () => derivePendingMessages(queuedMessages, atom.messages.map((m) => m.message_id)),
    [atom.messages, queuedMessages],
  );

  // Failed messages are rendered in InputArea with retry/dismiss controls.
  const failedMessages = useMemo(
    () => deriveFailedMessages(queuedMessages),
    [queuedMessages],
  );

  const connectionInfo = useConnection({
    conversationId: conversationIdForSSE,
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
            contextWindow: { used: 0 },
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

  // availableModels is populated by the shared useModels() poller above.

  // REQ-SEED-001: hydrate the input area from `seed-draft:<id>` localStorage
  // when a seeded conversation first mounts, then clear the key so revisits
  // don't re-hydrate it. We push the draft into InputArea via its imperative
  // `setDraft` handle, which routes through `useDraft` so persistence picks
  // up normally from there.
  const seedHydratedRef = useRef<string | null>(null);
  useEffect(() => {
    if (!conversationId) return;
    if (seedHydratedRef.current === conversationId) return;
    const key = `seed-draft:${conversationId}`;
    let draft: string | null = null;
    try {
      draft = localStorage.getItem(key);
    } catch {
      // ignore
    }
    if (!draft) return;
    seedHydratedRef.current = conversationId;
    // Defer to the next tick so InputArea has mounted and inputRef is set.
    const handle = window.setTimeout(() => {
      inputRef.current?.setDraft(draft!);
      try {
        localStorage.removeItem(key);
      } catch {
        // ignore
      }
    }, 0);
    return () => window.clearTimeout(handle);
  }, [conversationId]);

  // Auto-open/close task approval overlay on state transitions
  useEffect(() => {
    if (atom.phase.type === 'awaiting_task_approval') {
      setShowTaskApproval(true);
    } else {
      setShowTaskApproval(false);
    }
  }, [atom.phase.type]);

  // Ctrl+` toggles the terminal collapse state. Only blocked when focus is
  // inside the xterm itself — in every other input (chat textarea, etc.)
  // the shortcut should still work, matching how VS Code and iTerm2 behave.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (!e.ctrlKey || e.key !== '`') return;
      const active = document.activeElement as HTMLElement | null;
      if (active?.closest('.terminal-panel-xterm')) return;
      e.preventDefault();
      if (terminalPane.collapsed) {
        terminalPane.expandFromCollapsed();
      } else {
        terminalPane.setCollapsed(true);
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [terminalPane]);

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

  // Stable refs — needed inside sendMessage which is memoized with a stable
  // identity across renders.
  const markFailedRef = useRef(markFailed);
  useEffect(() => { markFailedRef.current = markFailed; }, [markFailed]);
  const dismissRef = useRef(dismiss);
  useEffect(() => { dismissRef.current = dismiss; }, [dismiss]);

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
          // Don't touch the queue here. The entry stays `pending` until
          // `atom.messages` contains a row with `message_id == localId`
          // (SSE echo), at which point `pendingMessages` filters it out
          // via the derivation above.
          //
          // Optimistic phase update: user pressed send, show awaiting_llm
          // immediately. The authoritative server-side phase change arrives
          // later via `sse_state_change` (with its own sequence_id) and
          // takes precedence. `local_phase_change` exists precisely to
          // carve out this "client-originated, not part of server total
          // order" action from the `applyIfNewer` guard (task 02675).
          dispatch({ type: 'local_phase_change', phase: { type: 'awaiting_llm' } });
        } else {
          // Offline path: hand the send off to the offline operation queue
          // for replay when connectivity returns. The entry stays in
          // `useMessageQueue` too — offline and online converge on the same
          // "wait for SSE echo to filter this out" rule. If we dropped it
          // from the queue here, the user would see the message vanish
          // during the offline window. (task 02676)
          await queueOperation({
            type: 'send_message',
            conversationId,
            payload: { text, images: imgs, localId },
            createdAt: new Date(),
            retryCount: 0,
            status: 'pending',
          });
        }
      } catch (err) {
        if (err instanceof ExpansionError) {
          // Don't mark as failed — InputArea restores the draft and shows
          // an inline error so the user can fix or remove the broken
          // @reference (REQ-IR-007). Keeping the message in the queue as
          // "failed" would duplicate it alongside the restored draft.
          dismissRef.current(localId);
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

  // Send queued messages when connection is restored. Iterate the derived
  // `pendingMessages` (NOT raw `queuedMessages`) so we don't re-POST entries
  // the server already has — those were filtered out by the derivation.
  useEffect(() => {
    if (!isConnected || !conversationId) return;

    for (const msg of pendingMessages) {
      if (sendingMessagesRef.current.has(msg.localId)) continue;
      sendMessageRef.current(msg.localId, msg.text, msg.images);
    }
  }, [isConnected, conversationId, pendingMessages]);

  const handleSend = async (text: string, attachedImages: ImageData[]) => {
    if (!conversationId) return;

    const msg = enqueue(text, attachedImages);

    if (isConnected) {
      // Await so expansion errors propagate back to InputArea (REQ-IR-007)
      await sendMessage(msg.localId, text, attachedImages);
    }
  };

  const handleRetry = useCallback((localId: string) => {
    const msg = queuedMessages.find((m) => m.localId === localId);
    if (!msg) return;

    // Populate the message back into the input area for review/editing
    // instead of directly resending (the banner truncates content and
    // the user may want to fix the issue that caused the failure).
    dismiss(localId);
    inputRef.current?.setDraft(msg.text);
  }, [queuedMessages, dismiss]);

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
    if (!conversationId) return;

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
      showInfo(`Switched to ${newModelId}`);
      dispatch({ type: 'local_conversation_update', updates: { model: newModelId } });
    } catch (err) {
      console.error('Failed to upgrade model:', err);
    }
  };

  // REQ-TERM-020 / REQ-SEED-001: "Let Phoenix set this up for me" handler.
  // TerminalPanel builds the prompt text and hands it off; this owns the API
  // call + navigation because it has conversationId, model, and router ctx.
  //
  // The seeded conversation is created with empty `text` — the backend
  // skips the initial UserMessage dispatch when `seed_parent_id` is set and
  // text is empty (handlers.rs). The new page hydrates its input area from
  // `seed-draft:<id>` in localStorage so the user can review and hit Send.
  const handleAssistShellSetup = useCallback(
    async (promptText: string, seedLabel: string, homeDir: string) => {
      if (!conversation?.id) return;
      const messageId =
        crypto.randomUUID?.() ??
        `seed-${Date.now()}-${Math.random().toString(36).slice(2)}`;
      // Stash the seed draft BEFORE navigation so it's visible to the new
      // page on first render (useDraft reads localStorage synchronously in
      // its initializer).
      const newConvPromise = api.createConversation(
        homeDir,
        '', // empty — server accepts empty text when seed_parent_id is set
        messageId,
        conversation.model ?? undefined,
        [],
        'direct',
        null,
        conversation.id,
        seedLabel,
      );
      const newConv = await newConvPromise;
      try {
        localStorage.setItem(`seed-draft:${newConv.id}`, promptText);
      } catch {
        // ignore — non-fatal
      }
      if (newConv.slug) {
        navigate(`/c/${newConv.slug}`);
      }
    },
    [conversation, navigate],
  );

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

  // REQ-SEED-003: click handler for the seed-parent breadcrumb link.
  // Defined here (before any conditional early returns) so the hook order is
  // stable across the !conversation / error branches below.
  const seedParentSlugForCallback = conversation?.seed_parent_slug;
  const handleSeedParentClick = useCallback((e: ReactMouseEvent) => {
    if (!seedParentSlugForCallback) return;
    e.preventDefault();
    navigate(`/c/${seedParentSlugForCallback}`);
  }, [seedParentSlugForCallback, navigate]);

  if (error) {
    return (
      <div id="app">
        <main id="main-area">
          <div className="empty-state">
            <div className="empty-state-icon"><XCircle /></div>
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

  // Narrow desktop (1025-1279px): the active viewer (prose reader OR
  // diff viewer) replaces conversation content as a full-screen pane.
  // Wide desktop (≥1280px) renders it as a split-pane sibling inside
  // the main return below (task 08654).
  if (isDesktop && !isWideDesktop) {
    if (fileExplorer.proseReaderState) {
      const prs = fileExplorer.proseReaderState;
      return (
        <div id="app">
          <Suspense fallback={null}>
            <ProseReader
              filePath={prs.path}
              rootDir={prs.rootDir}
              onClose={handleCloseProseReader}
              onSendNotes={handleSendNotes}
              patchContext={prs.patchContext ?? undefined}
              inline
            />
          </Suspense>
        </div>
      );
    }
    if (diffViewer.payload) {
      const dv = diffViewer.payload;
      return (
        <div id="app">
          <Suspense fallback={null}>
            <DiffView
              open
              comparator={dv.comparator}
              commitLog={dv.commit_log}
              committedDiff={dv.committed_diff}
              committedTruncatedKib={dv.committed_truncated_kib}
              committedSaturated={dv.committed_saturated}
              uncommittedDiff={dv.uncommitted_diff}
              uncommittedTruncatedKib={dv.uncommitted_truncated_kib}
              uncommittedSaturated={dv.uncommitted_saturated}
              onClose={handleCloseDiff}
              onSendNotes={handleSendNotes}
              inline
            />
          </Suspense>
        </div>
      );
    }
  }

  const convStateForChildren = atom.phase;
  const showTerminal =
    !!conversationId &&
    convStateForChildren.type !== 'terminal' &&
    convStateForChildren.type !== 'context_exhausted';

  // Derived: model context window is a pure function of the current model's
  // spec. Falls back to 200_000 when availableModels hasn't loaded yet or the
  // model isn't in the registry (matches prior denormalized default).
  const modelContextWindow =
    availableModels?.find((m) => m.id === atom.conversation?.model)?.context_window
    ?? 200_000;

  // REQ-SEED-003: seed parent breadcrumb. Rendered above the message list
  // when this conversation was spawned from another via a seed action.
  // If `seed_parent_slug` is present we link to it; if not (parent deleted),
  // we render unlinked text.
  // NB: `seedParentSlug` and `handleSeedParentClick` are defined up near the
  //     other `useCallback`s (before any conditional early returns) to keep
  //     hooks in a stable order.
  const seedBreadcrumb = conversation.seed_parent_id ? (
    <div className="conversation-seed-breadcrumb">
      {conversation.seed_parent_slug ? (
        <a href={`/c/${conversation.seed_parent_slug}`} onClick={handleSeedParentClick}>
          {'\u2190'} from: {conversation.seed_label ?? conversation.seed_parent_slug}
        </a>
      ) : (
        <span>
          {'\u2190'} from: {conversation.seed_label ?? '(parent deleted)'}
        </span>
      )}
    </div>
  ) : null;

  // Split-pane viewer: rendered inside `#app` as a sibling of
  // .conversation-column when wide-desktop and a viewer (file OR diff)
  // is open. CSS in .app-split-pane (index.css) flexes children
  // horizontally.
  const splitPanePrs = fileExplorer.proseReaderState;
  const splitPaneDiff = diffViewer.payload;
  const showSplitPaneViewer =
    isDesktop && isWideDesktop && (splitPanePrs !== null || splitPaneDiff !== null);

  return (
    <div
      id="app"
      className={showSplitPaneViewer ? 'app-split-pane' : undefined}
      style={
        showSplitPaneViewer
          ? ({ ['--viewer-pane-width' as string]: `${viewerPane.collapsed ? 0 : viewerPane.size}px` } as React.CSSProperties)
          : undefined
      }
    >
      <div className="conversation-column">
      {seedBreadcrumb}
      <MessageList
        messages={atom.messages}
        pendingMessages={pendingMessages}
        convState={convStateForChildren}
        onRetry={handleRetry}
        onOpenFile={handleOpenFileFromPatch}
        conversationId={conversationId}
        streamingBuffer={atom.streamingBuffer}
        systemPrompt={atom.systemPrompt ?? undefined}
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
        <div className={`context-exhausted-banner${contextExhaustedExpanded ? ' context-exhausted-banner--expanded' : ''}`}>
          <button
            type="button"
            className="context-exhausted-header"
            onClick={() => setContextExhaustedExpanded((v) => !v)}
            aria-expanded={contextExhaustedExpanded}
          >
            <span className="context-exhausted-icon"><AlertTriangle /></span>
            <span className="context-exhausted-title">Context Window Full</span>
            <span className="context-exhausted-subtitle">
              {conversation.continued_in_conv_id
                ? 'This conversation has been continued'
                : 'Continue in a new conversation to preserve progress'}
            </span>
            <span className={`context-exhausted-chevron${contextExhaustedExpanded ? ' context-exhausted-chevron--open' : ''}`} aria-hidden>
              <ChevronRightSmall />
            </span>
          </button>
          <div className="context-exhausted-summary">
            <div className="context-exhausted-actions">
              {conversation.continued_in_conv_id ? (
                // REQ-BED-030 single-continuation policy: once a parent has a
                // continuation, the Continue button is replaced with a link to
                // that continuation. Clicking re-hits the idempotent
                // continuation endpoint, which returns the existing id + slug
                // and lets us navigate without caching the slug client-side.
                <button
                  type="button"
                  className="context-exhausted-continue"
                  data-testid="continuation-link"
                  onClick={async () => {
                    if (!conversation?.id) return;
                    try {
                      const res = await api.continueConversation(conversation.id);
                      if (res.slug) {
                        navigate(`/c/${res.slug}`);
                      }
                    } catch (err) {
                      showInfo(err instanceof Error ? err.message : 'Failed to open continuation');
                    }
                  }}
                >
                  {'→'} Continued in a new conversation
                </button>
              ) : (
                <button
                  type="button"
                  className="context-exhausted-continue"
                  data-testid="continue-button"
                  onClick={async () => {
                    if (convStateForChildren.type !== 'context_exhausted') return;
                    if (!conversation?.id) return;
                    const summary = convStateForChildren.summary;
                    try {
                      const res = await api.continueConversation(conversation.id);
                      if (res.already_existed) {
                        showInfo('Returning to your existing continuation');
                      } else if (res.conversation_id && summary) {
                        // Pre-populate the continuation's input with the
                        // summary so the user can edit it before sending
                        // the first message. The seed-draft hydration
                        // useEffect on the new page picks this up and
                        // clears the key.
                        try {
                          localStorage.setItem(`seed-draft:${res.conversation_id}`, summary);
                        } catch {
                          // ignore storage failures — navigation still works
                        }
                      }
                      if (res.slug) {
                        navigate(`/c/${res.slug}`);
                      }
                    } catch (err) {
                      showInfo(err instanceof Error ? err.message : 'Failed to start new conversation');
                    }
                  }}
                >
                  Continue in new conversation
                </button>
              )}
              <button
                type="button"
                className="context-exhausted-copy"
                onClick={async () => {
                  if (convStateForChildren.type !== 'context_exhausted') return;
                  const ok = await copyToClipboard(convStateForChildren.summary);
                  showInfo(ok ? 'Summary copied to clipboard' : 'Copy failed -- select and copy manually');
                }}
              >
                Copy Summary
              </button>
              {!conversation.continued_in_conv_id &&
                (conversation.conv_mode_label === 'Work' ||
                  conversation.conv_mode_label === 'Branch') && (
                  // REQ-BED-031: abandon remains available on a context-exhausted
                  // parent as long as no continuation exists. Once continued, the
                  // abandon action belongs on the continuation. Only Work/Branch
                  // mode have a worktree to tear down — `abandon-task` rejects
                  // Explore/Direct with a 400, so the button only renders for
                  // modes that the API accepts.
                  <button
                    type="button"
                    className="context-exhausted-abandon"
                    data-testid="context-exhausted-abandon"
                    disabled={abandoningContextExhausted}
                    onClick={async () => {
                      if (!conversation?.id) return;
                      const isBranch = conversation.conv_mode_label === 'Branch';
                      const confirmed = window.confirm(
                        isBranch
                          ? 'Abandon this conversation? The worktree will be deleted but your branch will be kept.'
                          : 'Abandon this task? The worktree and task branch will be deleted.',
                      );
                      if (!confirmed) return;
                      setAbandoningContextExhausted(true);
                      try {
                        await api.abandonTask(conversation.id);
                      } catch (err) {
                        showInfo(err instanceof Error ? err.message : 'Failed to abandon task');
                      } finally {
                        setAbandoningContextExhausted(false);
                      }
                    }}
                  >
                    {abandoningContextExhausted ? 'Abandoning...' : 'Abandon'}
                  </button>
                )}
            </div>
            {contextExhaustedExpanded && (
              <pre className="context-exhausted-content">
                {convStateForChildren.summary}
              </pre>
            )}
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
      {convStateForChildren.type === 'awaiting_recovery' ? (
        <>
        {credentialStatus && (
          <Suspense fallback={null}>
            <CredentialHelperPanel
              active={true}
              onDismiss={() => void refreshModels().catch(() => {})}
            />
          </Suspense>
        )}
        <InputArea
          ref={inputRef}
          conversationId={conversationId}
          convState={convStateForChildren}
          images={images}
          setImages={setImages}
          isOffline={isOffline}
          failedMessages={failedMessages}
          convModeLabel={conversation.conv_mode_label}
          onSend={handleSend}
          onCancel={handleCancel}
          onRetry={handleRetry}
          onDismissError={dismiss}
        />
        </>
      ) : convStateForChildren.type === 'error' ? (
        <ErrorBanner
          message={convStateForChildren.message}
          onRetry={() => handleSend('continue', [])}
          onDismiss={() => dispatch({ type: 'local_phase_change', phase: { type: 'idle' } })}
        />
      ) : convStateForChildren.type === 'awaiting_user_response' ? (
        <QuestionPanel
          questions={convStateForChildren.questions}
          conversationId={conversation.id}
          showToast={showInfo}
          onSubmitted={() => dispatch({ type: 'local_phase_change', phase: { type: 'llm_requesting', attempt: 1 } })}
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
            continuedInConvId={conversation.continued_in_conv_id}
            onSendMessage={(text) => handleSend(text, [])}
          />
        )}
        {credentialStatus && credentialStatus !== 'not_configured' && credentialStatus !== 'valid' && (
          <Suspense fallback={null}>
            <CredentialHelperPanel
              active={showAuthPanel}
              onDismiss={() => {
                setShowAuthPanel(false);
                void refreshModels().catch(() => {});
              }}
            />
          </Suspense>
        )}
        <InputArea
          ref={inputRef}
          conversationId={conversationId}
          convState={convStateForChildren}
          images={images}
          setImages={setImages}
          isOffline={isOffline}
          failedMessages={failedMessages}
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
        modelContextWindow={modelContextWindow}
        availableModels={availableModels}
        onRetryNow={connectionInfo.retryNow}
        onTriggerContinuation={handleTriggerContinuation}
        onUpgradeModel={handleUpgradeModel}
        toolExecutingStartedAt={atom.toolExecutingStartedAt}
      />
      </div>

      {/* Terminal split-pane (REQ-TERM-001) — collapsed = 32px header strip.
          Lazy-loaded so xterm (~200KB) stays out of the main bundle. */}
      {showTerminal && (
        <>
          <PaneDivider
            orientation="horizontal"
            title="Drag to resize • Double-click to collapse/expand"
            onPointerDown={(e) => terminalPane.startDrag(e, 'y', true)}
            onDoubleClick={() => {
              if (terminalPane.collapsed) {
                terminalPane.expandFromCollapsed();
              } else {
                terminalPane.setCollapsed(true);
              }
            }}
          />
          <Suspense fallback={null}>
            <TerminalPanel
              conversationId={conversationId!}
              height={terminalPane.collapsed ? TERMINAL_COLLAPSED_PX : terminalPane.size}
              collapsed={terminalPane.collapsed}
              onExpand={terminalPane.expandFromCollapsed}
              onCollapse={() => terminalPane.setCollapsed(true)}
              cwd={conversation.cwd}
              shell={conversation.shell ?? undefined}
              homeDir={conversation.home_dir ?? undefined}
              onAssistSetup={handleAssistShellSetup}
            />
          </Suspense>
        </>
      )}

      {/* Task approval overlay — browser back navigates away; SSE restores state on return. */}
      {showTaskApproval && atom.phase.type === 'awaiting_task_approval' && (
        <Suspense fallback={null}>
          <TaskApprovalReader
            title={atom.phase.title}
            priority={atom.phase.priority}
            plan={atom.phase.plan}
            onApprove={handleApproveTask}
            onReject={handleRejectTask}
            onSendFeedback={handleTaskFeedback}
          />
        </Suspense>
      )}

      <Toast messages={toasts} onDismiss={dismissToast} />

      {/* First task welcome modal */}
      {showFirstTaskWelcome && (
        <Suspense fallback={null}>
          <FirstTaskWelcome
            visible={showFirstTaskWelcome}
            onClose={() => setShowFirstTaskWelcome(false)}
          />
        </Suspense>
      )}


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
        <Suspense fallback={null}>
          <ProseReader
            filePath={mobileProseFile.path}
            rootDir={mobileProseFile.rootDir}
            onClose={handleCloseProseReader}
            onSendNotes={handleSendNotes}
            patchContext={mobileProseFile.patchContext ?? undefined}
          />
        </Suspense>
      )}
      {/* Diff overlay: rendered as a full-screen overlay whenever the
          diff viewer is open AND the split pane isn't (mobile, narrow
          desktop, or any future case where the split is unavailable). */}
      {diffViewer.payload && !showSplitPaneViewer && (
        <Suspense fallback={null}>
          <DiffView
            open
            comparator={diffViewer.payload.comparator}
            commitLog={diffViewer.payload.commit_log}
            committedDiff={diffViewer.payload.committed_diff}
            committedTruncatedKib={diffViewer.payload.committed_truncated_kib}
            committedSaturated={diffViewer.payload.committed_saturated}
            uncommittedDiff={diffViewer.payload.uncommitted_diff}
            uncommittedTruncatedKib={diffViewer.payload.uncommitted_truncated_kib}
            uncommittedSaturated={diffViewer.payload.uncommitted_saturated}
            onClose={handleCloseDiff}
            onSendNotes={handleSendNotes}
          />
        </Suspense>
      )}
      {showSplitPaneViewer && (
        <>
          <div
            className="viewer-pane-divider"
            title="Drag to resize the viewer pane • Double-click to collapse"
            role="separator"
            aria-orientation="vertical"
            aria-label="Resize viewer pane"
            aria-valuemin={VIEWER_PANE_MIN}
            aria-valuemax={VIEWER_PANE_MAX}
            aria-valuenow={viewerPane.collapsed ? 0 : viewerPane.size}
            tabIndex={0}
            onPointerDown={(e) => viewerPane.startDrag(e, 'x')}
            onDoubleClick={() => viewerPane.setCollapsed(!viewerPane.collapsed)}
            onKeyDown={(e) => {
              // Keyboard resize for the WAI-ARIA `separator` pattern.
              // ArrowLeft / ArrowRight nudge ±32px; Home / End clamp
              // to min / max; Enter / Space toggle collapse. setSize
              // applies the same clamp the drag path uses.
              const STEP = 32;
              if (e.key === 'ArrowLeft') {
                e.preventDefault();
                viewerPane.setSize(viewerPane.size + STEP);
              } else if (e.key === 'ArrowRight') {
                e.preventDefault();
                viewerPane.setSize(viewerPane.size - STEP);
              } else if (e.key === 'Home') {
                e.preventDefault();
                viewerPane.setSize(VIEWER_PANE_MAX);
              } else if (e.key === 'End') {
                e.preventDefault();
                viewerPane.setSize(VIEWER_PANE_MIN);
              } else if (e.key === 'Enter' || e.key === ' ') {
                e.preventDefault();
                viewerPane.setCollapsed(!viewerPane.collapsed);
              }
            }}
          />
          <div className="conversation-viewer-pane">
            <Suspense fallback={null}>
              {splitPaneDiff ? (
                <DiffView
                  open
                  comparator={splitPaneDiff.comparator}
                  commitLog={splitPaneDiff.commit_log}
                  committedDiff={splitPaneDiff.committed_diff}
                  committedTruncatedKib={splitPaneDiff.committed_truncated_kib}
                  committedSaturated={splitPaneDiff.committed_saturated}
                  uncommittedDiff={splitPaneDiff.uncommitted_diff}
                  uncommittedTruncatedKib={splitPaneDiff.uncommitted_truncated_kib}
                  uncommittedSaturated={splitPaneDiff.uncommitted_saturated}
                  onClose={handleCloseDiff}
                  onSendNotes={handleSendNotes}
                  inline
                />
              ) : splitPanePrs ? (
                <ProseReader
                  filePath={splitPanePrs.path}
                  rootDir={splitPanePrs.rootDir}
                  onClose={handleCloseProseReader}
                  onSendNotes={handleSendNotes}
                  patchContext={splitPanePrs.patchContext ?? undefined}
                  inline
                />
              ) : null}
            </Suspense>
          </div>
        </>
      )}
    </div>
  );
}
