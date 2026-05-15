import { lazy, Suspense, useState, useEffect, useRef, useCallback, useMemo, type MouseEvent as ReactMouseEvent, type ReactNode } from 'react';
import { useParams, useNavigate } from 'react-router-dom';
import { api, ExpansionError, type Conversation, type ImageData } from '../api';
import { refreshModels } from '../modelsPoller';
import { isAgentWorking, isCancellingState, parseConversationState } from '../utils';
import { copyToClipboard } from '../utils/clipboard';
import { cacheDB } from '../cache';
import { MessageList } from '../components/MessageList';
import { ConnectedInputArea } from '../components/InputArea';
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
import { useConversationAtom, useConversationSnapshot, useCreateConversationWithStore } from '../conversation';
import {
  useResizablePane,
  useIsDesktop,
  useIsWideDesktop,
  useDraftActions,
  DraftLifecycle,
} from '../hooks';

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
const BrowserViewPanel = lazy(() =>
  import('../components/BrowserViewPanel').then((m) => ({ default: m.BrowserViewPanel })),
);

import { ReviewNotesProvider } from '../contexts/ReviewNotesContext';
import {
  BrowserViewStateProvider,
  DiffViewerStateProvider,
  useBrowserViewState,
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
  const { slug } = useParams<{ slug: string }>();
  return (
    <ReviewNotesProvider scopeKey={slug}>
      <DiffViewerStateProvider scopeKey={slug}>
        <BrowserViewWrapper slug={slug}>
          {/*
            DraftLifecycle hosts the localStorage hydration and debounced
            write-through for the active conversation's draft. Mounted
            here — above ConversationPageContent's fullscreen viewer
            early-returns (ProseReader, Diff, Browser) — so the
            persistence subscription survives every composer
            unmount/remount cycle. If it lived inside the main-chat JSX,
            switching to a fullscreen viewer would unmount it and any
            external draft mutation (e.g. `phoenix:insert-draft`) would
            never reach localStorage until the user returned to chat.
          */}
          {slug && <DraftLifecycle slug={slug} />}
          <ConversationPageContent />
        </BrowserViewWrapper>
      </DiffViewerStateProvider>
    </ReviewNotesProvider>
  );
}

/** Bridges the conversation atom's `browser_session_active` flag into the
 *  `BrowserViewStateProvider`. Sits above `ConversationPageContent` so the
 *  provider can be a thin pass-through. `useConversationSnapshot` returns
 *  the same `Conversation` reference across renders unless the row itself
 *  changes, so this wrapper does not re-render on token churn. */
function BrowserViewWrapper({
  slug,
  children,
}: {
  slug: string | undefined;
  children: ReactNode;
}) {
  const conversation = useConversationSnapshot(slug ?? null);
  return (
    <BrowserViewStateProvider
      scopeKey={slug}
      browserSessionActive={conversation?.browser_session_active ?? false}
    >
      {children}
    </BrowserViewStateProvider>
  );
}

function ConversationPageContent() {
  const { slug } = useParams<{ slug: string }>();
  const navigate = useNavigate();
  const createConversationWithStore = useCreateConversationWithStore();

  // Atom-backed conversation state (survives navigation via ConversationProvider)
  const [atom, dispatch] = useConversationAtom(slug!);

  // Derived from atom
  const conversationId = atom.conversationId ?? undefined;
  const conversation = atom.conversation;

  // Page-level state — not conversation data
  const [error, setError] = useState<string | null>(null);

  // File explorer context (shared with desktop panel)
  const fileExplorer = useFileExplorer();
  // Diff viewer slot — lifted out of WorkActions so the diff can mount
  // inline beside chat at ≥1280px (task 08654 follow-on).
  const diffViewer = useDiffViewerState();
  // Browser live-view slot (REQ-BT-018). Mutually exclusive with prose +
  // diff. Auto-mount logic lives below in the messages effect.
  const browserView = useBrowserViewState();
  // Single-slot model: opening one viewer closes the other so the user
  // never sees both fighting for the split pane. When both are set,
  // file wins (most-recent-action — fileExplorer.openFile is what
  // triggered this collision since the user just clicked a file). The
  // alternate ordering (user clicks View Diff while file is open)
  // closes the file via fileExplorer.closeFile in the click handler
  // chain elsewhere; this effect catches the file-clicks-while-diff-open
  // case the click handlers don't reach.
  const closeDiff = diffViewer.close;
  const closeBrowserView = browserView.closePanel;
  useEffect(() => {
    if (fileExplorer.proseReaderState && diffViewer.payload) {
      closeDiff();
    }
  }, [fileExplorer.proseReaderState, diffViewer.payload, closeDiff]);
  // Browser view is mutually exclusive with prose + diff. If anything else
  // is in the slot, close the browser view; the user's most recent action
  // wins. The reverse direction (closing prose/diff when the user opens
  // browser) is handled in `handleOpenBrowserView` below.
  useEffect(() => {
    if (browserView.open && (fileExplorer.proseReaderState || diffViewer.payload)) {
      closeBrowserView();
    }
  }, [browserView.open, fileExplorer.proseReaderState, diffViewer.payload, closeBrowserView]);
  // Close handlers also clear the OTHER viewer to be safe (for cases
  // where state machines briefly hold both during transitions).
  const handleCloseDiff = useCallback(() => closeDiff(), [closeDiff]);
  const handleCloseBrowserView = useCallback(() => closeBrowserView(), [closeBrowserView]);
  const handleOpenBrowserView = useCallback(() => {
    fileExplorer.closeFile();
    closeDiff();
    browserView.openPanel();
  }, [fileExplorer, closeDiff, browserView]);
  // ConversationPage was previously snapshotting `isDesktop` at mount and
  // never resubscribing — a window resize across 1025px wouldn't update
  // the layout until the user navigated. The shared hooks now subscribe
  // on every consumer.
  const isDesktop = useIsDesktop();
  // Wider threshold (≥1280px) gates the split-pane prose reader (task 08654).
  // Below this we keep the existing full-screen overlay UX; above, the
  // reader sits beside the chat as a resizable sibling pane.
  const isWideDesktop = useIsWideDesktop();
  const VIEWER_PANE_MIN = 360;
  const VIEWER_PANE_MAX = 1200;
  const viewerPane = useResizablePane({
    key: 'viewer-pane-width',
    min: VIEWER_PANE_MIN,
    max: VIEWER_PANE_MAX,
    defaultSize: 600,
    collapseThreshold: 280,
  });

  // Mobile-only file browser overlay. The prose reader itself reads its
  // open-file state from `fileExplorer.proseReaderState` (URL-driven), so
  // mobile and desktop share a single source of truth — opening or closing
  // a file just rewrites `?file=...&root=...` on the current URL, which
  // means an iOS PWA cold reload restores the exact view.
  const [showFileBrowser, setShowFileBrowser] = useState(false);

  const sendingMessagesRef = useRef<Set<string>>(new Set());
  const inputRef = useRef<InputAreaHandle>(null);

  // Page-scope draft action handles. `useDraftActions` is a memoized
  // dispatcher over the slug-keyed DraftStore — no atom subscription, no
  // value subscription, so ConversationPage does not re-render on
  // keystrokes. The draft VALUE subscription lives inside
  // `<ConnectedInputArea>` (composer subtree only); persistence lives in
  // `<DraftLifecycle>` (returns null — mounted at page level).
  const { setDraft: setDraftCb, appendDraft: appendDraftCb } = useDraftActions(slug!);

  // Monotonic focus-request counter. Any time we mutate the draft from
  // outside the textarea (terminal selection, prose-reader notes, retry,
  // seed hydration, skill insert), we bump this so InputArea's focus
  // effect fires — including across an unmount/remount on narrow viewports.
  // Reset on slug change in the per-slug reset block below — otherwise a
  // non-zero token from the previous conversation would steal focus on the
  // next remount of <InputArea> without an explicit request.
  const [focusToken, setFocusToken] = useState(0);
  const requestComposerFocus = useCallback(() => {
    setFocusToken((t) => t + 1);
  }, []);

  // App state for offline support
  const { isOnline, queueOperation } = useAppMachine();

  // Toast for question panel feedback
  const { toasts, dismissToast, showInfo, showError } = useToast();

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

  // ---------------------------------------------------------------------------
  // Per-slug state reset (task 02703).
  //
  // Previously KeyedConversationPage used `key={slug}` to force a fresh React
  // tree on every conversation change. That caused the entire content area to
  // flash blank on every navigation. We instead keep the page mounted across
  // slug changes and reset the per-conversation state explicitly here.
  //
  // The reset is synchronous: when `slug` differs from `lastSlug`, we update
  // `lastSlug` AND reset every per-conversation useState in the same render
  // pass ("adjusting state during render":
  // https://react.dev/learn/you-might-not-need-an-effect#adjusting-some-state-when-a-prop-changes).
  // React detects this and re-renders before commit, so the first paint after
  // a slug change shows clean state — NEVER content from the previous
  // conversation. Honest UI: if the new conversation's data isn't ready, the
  // `if (!conversation)` early-return below paints a clean skeleton.
  //
  // Refs (sendingMessagesRef, seedHydratedRef, cachedMsgCountRef) are also
  // reset here. Mutating .current during render is safe because refs don't
  // trigger re-renders. The refs live alongside this block (vs. their
  // original declaration sites further down the file) so the reset can see
  // them and the contract "these are per-slug state" is colocated.
  const seedHydratedRef = useRef<string | null>(null);
  const cachedMsgCountRef = useRef(0);
  const [lastSlug, setLastSlug] = useState<string | undefined>(slug);
  if (lastSlug !== slug) {
    setLastSlug(slug);
    // useState resets — React batches these into the same render.
    setError(null);
    setShowFileBrowser(false);
    setImages([]);
    setShowTaskApproval(false);
    setShowFirstTaskWelcome(false);
    setContextExhaustedExpanded(true);
    setAbandoningContextExhausted(false);
    setFocusToken(0);
    // Ref resets — immediate, no re-render.
    sendingMessagesRef.current = new Set();
    seedHydratedRef.current = null;
    cachedMsgCountRef.current = 0;
  }
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
  const { queuedMessages, enqueue, markFailed, markSteeringQueued, dismiss } =
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
    conversationId,
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
                  : result.presentation_mode === 'working'
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

  // Fetch system prompt once when conversationId is known
  useEffect(() => {
    if (!conversationId) return;
    api
      .getSystemPrompt(conversationId)
      .then((sp) => dispatch({ type: 'set_system_prompt', systemPrompt: sp, expectedConversationId: conversationId }))
      .catch((err) => console.warn('Failed to load system prompt:', err));
  }, [conversationId, dispatch]);

  // availableModels is populated by the shared useModels() poller above.

  // REQ-SEED-001: hydrate the draft from `seed-draft:<id>` localStorage when
  // a seeded conversation first mounts, then clear the key so revisits don't
  // re-hydrate it. Dispatches through `setDraftCb` (which dispatches
  // `set_draft` on the conversation atom); the persistence side-effect in
  // `useDraft` mirrors the value to `phoenix:draft:<id>` after that.
  // (`seedHydratedRef` is declared with the per-slug reset block above so it
  //  resets to null on slug change.)
  useEffect(() => {
    if (!conversationId) return;
    if (seedHydratedRef.current === conversationId) return;
    const key = `seed-draft:${conversationId}`;
    let seed: string | null = null;
    try {
      seed = localStorage.getItem(key);
    } catch {
      // ignore
    }
    if (!seed) return;
    seedHydratedRef.current = conversationId;
    setDraftCb(seed);
    requestComposerFocus();
    try {
      localStorage.removeItem(key);
    } catch {
      // ignore
    }
  }, [conversationId, setDraftCb, requestComposerFocus]);

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

  // Cache new messages as they arrive via SSE.
  // (`cachedMsgCountRef` is declared with the per-slug reset block above so
  //  it resets to 0 on slug change.)
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

  // REQ-BT-018: react to edges of the server-authoritative
  // `browser_session_active` flag. Rising edge (false→true) auto-mounts
  // the live view if the slot is empty. Falling edge (true→false) closes
  // the panel so the user isn't left staring at a stale "No browser yet"
  // overlay after a kill / idle-cleanup. The `prevRef` is seeded with the
  // current value so a page that mounts with `active === true` does NOT
  // trigger auto-open — only in-page transitions do.
  const browserSessionActive = browserView.browserSessionActive;
  const openBrowserPanel = browserView.openPanel;
  const prevBrowserSessionActiveRef = useRef(browserSessionActive);
  useEffect(() => {
    const wasActive = prevBrowserSessionActiveRef.current;
    prevBrowserSessionActiveRef.current = browserSessionActive;
    if (!wasActive && browserSessionActive) {
      if (!fileExplorer.proseReaderState && !diffViewer.payload) {
        openBrowserPanel();
      }
    } else if (wasActive && !browserSessionActive) {
      closeBrowserView();
    }
  }, [
    browserSessionActive,
    openBrowserPanel,
    closeBrowserView,
    fileExplorer.proseReaderState,
    diffViewer.payload,
  ]);

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
          const result = await api.sendMessage(conversationId, text, imgs, localId);
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
          if (result.steering) {
            // Conversation was busy — message queued server-side for delivery
            // when the conversation next reaches Idle. Show a "Queued" pill
            // on the message bubble instead of the normal sending spinner.
            markSteeringQueued(localId);
            // No phase change: the conversation is already running.
          } else {
            dispatch({ type: 'local_phase_change', phase: { type: 'awaiting_llm' }, expectedConversationId: conversationId });
          }
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
    [conversationId, isOnline, queueOperation, dispatch, markSteeringQueued]
  );

  const sendMessageRef = useRef(sendMessage);
  useEffect(() => { sendMessageRef.current = sendMessage; }, [sendMessage]);

  // Send queued messages when connection is restored. Iterate the derived
  // `pendingMessages` (NOT raw `queuedMessages`) so we don't re-POST entries
  // the server already has — those were filtered out by the derivation.
  // Skip `steering_queued` messages — they are already held server-side and
  // will be delivered automatically when the conversation reaches Idle.
  useEffect(() => {
    if (!isConnected || !conversationId) return;

    for (const msg of pendingMessages) {
      if (msg.status === 'steering_queued') continue;
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
    setDraftCb(msg.text);
    requestComposerFocus();
  }, [queuedMessages, dismiss, setDraftCb, requestComposerFocus]);

  const handleCancel = async () => {
    if (!conversationId || !isAgentWorking(atom.phase)) return;
    if (isCancellingState(atom.phase)) return;

    try {
      await api.cancelConversation(conversationId);
    } catch (err) {
      console.error('Failed to cancel:', err);
    }
  };

  const handleCancelSteering = useCallback(async (localId: string) => {
    if (!conversationId) return;
    try {
      await api.cancelSteeringMessage(conversationId, localId);
      dismiss(localId);
    } catch (err) {
      console.error('Failed to cancel steering message:', err);
    }
  }, [conversationId, dismiss]);

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
      dispatch({ type: 'local_conversation_update', updates: { model: newModelId }, expectedConversationId: conversationId });
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
      const newConv = await createConversationWithStore(
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
      try {
        localStorage.setItem(`seed-draft:${newConv.id}`, promptText);
      } catch {
        // ignore — non-fatal
      }
      if (newConv.slug) {
        navigate(`/c/${newConv.slug}`);
      }
    },
    [conversation, navigate, createConversationWithStore],
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
      fileExplorer.openFile(filePath, rootDir);
    },
    [fileExplorer]
  );

  const handleCloseProseReader = useCallback(() => {
    fileExplorer.closeFile();
  }, [fileExplorer]);

  // Task 02672: terminal selection → composer draft.
  // TerminalPanel fires this when the user presses Cmd/Ctrl+Shift+L with
  // text selected. We fence the selection so it stays distinguishable from
  // the user's in-flight prose, prefix it with a label that names the source
  // (tmux pane when the conversation is tmux-backed, plain "terminal"
  // otherwise) plus the cwd, append to the existing draft (never replace),
  // and focus the composer so the user can immediately type a follow-up.
  //
  // Naming the tmux pane (`main:1.0` — first window 1 since `base-index 1`,
  // first pane 0; see crates/phoenix-ide/src/tools/tmux/server.conf) is
  // deliberate: the LLM can then call the existing `tmux` tool (e.g.
  // `capture-pane -p -t main:1.0 -S -200`) to pull the rest of the pane
  // on follow-up — Phoenix injects the correct socket so no further
  // coordinates are needed. Drifts if the user splits the pane or opens
  // additional windows; threading the live pane id through SSE is a
  // future refinement.
  const handleSendTerminalSelection = useCallback(
    (selection: string) => {
      if (!selection) return;
      const trimmed = selection.replace(/\s+$/u, '');
      if (!trimmed) return;
      const cwdHint = conversation?.cwd ? ` (cwd: \`${conversation.cwd}\`)` : '';
      const sourceLabel = conversation?.terminal_uses_tmux
        ? 'From tmux pane `main:1.0`'
        : 'From terminal';
      // Fence length must exceed the longest backtick run in the selection
      // (CommonMark §4.5) so output containing literal triple-backticks —
      // markdown snippets, AI tool transcripts — doesn't close the fence
      // early.
      let longestRun = 0;
      let run = 0;
      for (let i = 0; i < trimmed.length; i++) {
        if (trimmed.charCodeAt(i) === 0x60 /* ` */) {
          run += 1;
          if (run > longestRun) longestRun = run;
        } else {
          run = 0;
        }
      }
      const fence = '`'.repeat(Math.max(3, longestRun + 1));
      // Trailing `\n` keeps the closing fence on its own line per
      // CommonMark §4.5 — without it, the next character the user types
      // lands on the fence's line and extends the code block.
      const fenced = `${sourceLabel}${cwdHint}:\n${fence}\n${trimmed}\n${fence}\n`;
      appendDraftCb(fenced);
      requestComposerFocus();
    },
    [conversation?.cwd, conversation?.terminal_uses_tmux, appendDraftCb, requestComposerFocus]
  );

  // External "set this as the draft" trigger fired by surfaces that don't
  // hold a ref to the composer (skill viewer, message context menu). With
  // draft state in the atom, this is a thin shim: dispatch + focus token.
  // Replaces the prior InputArea-local listener that only worked when
  // InputArea happened to be mounted.
  useEffect(() => {
    const handler = (e: Event) => {
      const text = (e as CustomEvent<{ text: string }>).detail?.text;
      if (!text) return;
      setDraftCb(text);
      requestComposerFocus();
    };
    window.addEventListener('phoenix:insert-draft', handler);
    return () => window.removeEventListener('phoenix:insert-draft', handler);
  }, [setDraftCb, requestComposerFocus]);

  const handleSendNotes = useCallback(
    (formattedNotes: string) => {
      // Draft lives in the conversation atom, so this works the same whether
      // `<InputArea>` is currently mounted (right-pane / mobile-overlay flow)
      // or unmounted (narrow-desktop fullscreen flow — the viewer closes
      // immediately below). `requestComposerFocus()` is a token bump that
      // InputArea consumes on its next render, including after a remount.
      appendDraftCb(formattedNotes);
      requestComposerFocus();
      fileExplorer.closeFile();
    },
    [fileExplorer, appendDraftCb, requestComposerFocus]
  );

  const handleOpenFileFromPatch = useCallback(
    (filePath: string, modifiedLines: Set<number>, firstModifiedLine: number) => {
      const rootDir = conversation?.cwd || '/';
      const fullPath = filePath.startsWith('/') ? filePath : `${rootDir}/${filePath}`;
      fileExplorer.openFile(fullPath, rootDir, { modifiedLines, firstModifiedLine });
    },
    [conversation?.cwd, fileExplorer]
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
    if (browserView.open && conversationId) {
      return (
        <div id="app">
          <Suspense fallback={null}>
            <BrowserViewPanel
              conversationId={conversationId}
              onClose={handleCloseBrowserView}
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
  const splitPaneBrowser = browserView.open;
  const showSplitPaneViewer =
    isDesktop
    && isWideDesktop
    && (splitPanePrs !== null || splitPaneDiff !== null || splitPaneBrowser);

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
      {browserView.browserSessionActive && !browserView.open && (
        <div className="browser-view-launcher">
          <button
            type="button"
            className="browser-view-launcher-btn"
            data-testid="browser-view-launcher"
            onClick={handleOpenBrowserView}
            title="Show live browser view"
          >
            ◍ Browser
          </button>
        </div>
      )}
      <MessageList
        messages={atom.messages}
        pendingMessages={pendingMessages}
        convState={convStateForChildren}
        onRetry={handleRetry}
        onCancelSteering={handleCancelSteering}
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
        <ConnectedInputArea
          ref={inputRef}
          slug={slug!}
          conversationId={conversationId}
          convState={convStateForChildren}
          images={images}
          setImages={setImages}
          isOffline={isOffline}
          failedMessages={failedMessages}
          convModeLabel={conversation.conv_mode_label}
          focusToken={focusToken}
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
          onDismiss={() => dispatch({ type: 'local_phase_change', phase: { type: 'idle' }, expectedConversationId: conversation.id })}
        />
      ) : convStateForChildren.type === 'awaiting_user_response' ? (
        <QuestionPanel
          questions={convStateForChildren.questions}
          conversationId={conversation.id}
          showToast={showInfo}
          onSubmitted={() => dispatch({ type: 'local_phase_change', phase: { type: 'llm_requesting', attempt: 1 }, expectedConversationId: conversation.id })}
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
        <ConnectedInputArea
          ref={inputRef}
          slug={slug!}
          conversationId={conversationId}
          convState={convStateForChildren}
          images={images}
          setImages={setImages}
          isOffline={isOffline}
          failedMessages={failedMessages}
          convModeLabel={conversation.conv_mode_label}
          focusToken={focusToken}
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
        onOpenFiles={isDesktop ? undefined : () => setShowFileBrowser(true)}
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
              showError={showError}
              onSendSelectionToDraft={handleSendTerminalSelection}
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

      {/* Mobile prose reader overlay — reads URL-driven state from
          FileExplorerProvider so cold reload (e.g. iOS PWA return) restores
          the exact file the user was viewing. */}
      {!isDesktop && fileExplorer.proseReaderState && (
        <Suspense fallback={null}>
          <ProseReader
            filePath={fileExplorer.proseReaderState.path}
            rootDir={fileExplorer.proseReaderState.rootDir}
            onClose={handleCloseProseReader}
            onSendNotes={handleSendNotes}
            patchContext={fileExplorer.proseReaderState.patchContext ?? undefined}
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
      {/* Browser view overlay: same fallback role as the diff overlay above
          — mobile, narrow desktop, or any case where the split pane is
          unavailable. REQ-BT-018. */}
      {browserView.open && !showSplitPaneViewer && conversationId && (
        <Suspense fallback={null}>
          <div className="browser-view-overlay">
            <BrowserViewPanel
              conversationId={conversationId}
              onClose={handleCloseBrowserView}
            />
          </div>
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
              ) : splitPaneBrowser && conversationId ? (
                <BrowserViewPanel
                  conversationId={conversationId}
                  onClose={handleCloseBrowserView}
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
