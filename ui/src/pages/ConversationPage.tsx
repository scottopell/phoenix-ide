import { lazy, Suspense, useState, useEffect, useLayoutEffect, useRef, useCallback, useMemo, useReducer, type MouseEvent as ReactMouseEvent } from 'react';
import { useParams, useNavigate, useLocation } from 'react-router-dom';
import { api, canChangeModelInState, isTerminalConversationState, ExpansionError, MessageSliceAlignmentError, type Conversation, type FileAttachment, type ImageData, type Message } from '../api';
import { refreshModels } from '../modelsPoller';
import { canCancelConversationState, isCancellingState, parseConversationState } from '../utils';
import { copyToClipboard } from '../utils/clipboard';
import { generateUUID } from '../utils/uuid';
import { cacheDB } from '../cache';
import { terminalPaneStorageKey } from '../storage/terminalPaneStorage';
import { canShowCommissionReviewViewer } from './commissionReviewViewerPrecedence';
import { ConversationNavStack } from '../components/ConversationNavStack';
import {
  historyMergeEventCursorFloor,
  initialHistoryExpansionState,
  reduceHistoryExpansion,
  type HistoryIntent,
  type HistoryScrollCommand,
  type RestoreBasis,
} from '../conversation/historyExpansion';
import { transcriptPositioningInputFromHistoryExpansion } from '../conversation/transcriptPositioning';
import { messageCacheWrite } from '../conversation/messageCachePersistence';
import {
  buildHistoricalUnits,
  findHistoricalUnitIndexByMessageId,
} from '../conversation/renderUnits';
import { ConnectedInputArea } from '../components/InputArea';
import type { InputAreaHandle } from '../components/InputArea';
import { ExploreOnboardingBanner } from '../components/ExploreOnboardingBanner';
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
  useConversationPrStatus,
  DraftLifecycle,
} from '../hooks';
import { useToast } from '../hooks/useToast';
import { Toast } from '../components/Toast';
import { useAppMachine } from '../hooks/useAppMachine';
import { ConnectedStateBar } from '../components/StateBar';
import { OPEN_MESSAGE_VIEWER_EVENT } from '../components/MessageContextMenu';
import { RenderProfiler } from '../dev/renderProfiler';
import { ErrorBanner } from '../components/ErrorBanner';
import { WorkControlBar } from '../components/WorkActions';
import {
  useConversationEventCursorRef,
  useConversationView,
  useWorkScope,
  useCreateConversationWithStore,
  readCreateIntent,
  clearCreateIntent,
} from '../conversation';
import {
  useResizablePane,
  useIsDesktop,
  useIsWideDesktop,
  useDraftActions,
} from '../hooks';

// Conditional overlays / heavy panels — code-split so the default render path
// (chat view with no overlay open) doesn't pay their bundle cost.
// - FileViewer + MetaViewer bodies, TaskApprovalReader: pull in react-syntax-highlighter
// - TerminalPanel: pulls in xterm + addon (large)
// - CredentialHelperPanel, FirstTaskWelcome: rarely mounted
const FileViewer = lazy(() =>
  import('../components/FileViewer').then((m) => ({ default: m.FileViewer })),
);
const ConversationDiffViewer = lazy(() =>
  import('../components/viewer/ConversationDiffViewer').then((m) => ({ default: m.ConversationDiffViewer })),
);
const TaskApprovalReader = lazy(() =>
  import('../components/TaskApprovalReader').then((m) => ({ default: m.TaskApprovalReader })),
);
const CommissionReviewApproval = lazy(() =>
  import('../components/CommissionReviewApproval').then((m) => ({ default: m.CommissionReviewApproval })),
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
const ProcessInspectorPanel = lazy(() =>
  import('../components/ProcessInspectorPanel').then((m) => ({ default: m.ProcessInspectorPanel })),
);
const MessageViewer = lazy(() =>
  import('../components/MessageViewer').then((m) => ({ default: m.MessageViewer })),
);
const CommissionReviewViewer = lazy(() =>
  import('../features/commissionReview/CommissionReviewViewer').then((m) => ({ default: m.CommissionReviewViewer })),
);

import { ReviewNotesProvider } from '../contexts/ReviewNotesContext';
import { useViewerSlot } from '../contexts/ViewerSlotContext';
import { useConversationReadiness } from '../contexts/useConversationReadiness';
import {
  ForkProposalsProvider,
  useForkProposals,
  type ForkActionOutcome,
} from '../contexts/ForkProposalsContext';

const ForkProposalReview = lazy(() =>
  import('../components/ForkProposalReview').then((m) => ({ default: m.ForkProposalReview })),
);

const TERMINAL_COLLAPSED_PX = 32;
const terminalPaneMax = () => Math.min(800, Math.floor(window.innerHeight * 0.75));

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
const routeForConversation = (conv: { id: string; slug?: string | null }) => `/c/${conv.id}`;
const UUID_ROUTE_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const isUuidRouteSegment = (segment: string) => UUID_ROUTE_RE.test(segment);

function prefersConversationIdRoute(routeSegment: string): boolean {
  return isUuidRouteSegment(routeSegment);
}

async function getCachedConversationForRoute(routeSegment: string): Promise<Conversation | null> {
  return prefersConversationIdRoute(routeSegment)
    ? await cacheDB.getConversation(routeSegment) ?? await cacheDB.getConversationBySlug(routeSegment)
    : await cacheDB.getConversationBySlug(routeSegment) ?? await cacheDB.getConversation(routeSegment);
}

async function getConversationForRoute(routeSegment: string) {
  if (prefersConversationIdRoute(routeSegment)) {
    try {
      return await api.getConversation(routeSegment);
    } catch (err) {
      if (!(err instanceof Error) || err.message !== 'Conversation not found') throw err;
      return api.getConversationBySlug(routeSegment);
    }
  }
  try {
    return await api.getConversationBySlug(routeSegment);
  } catch (err) {
    if (!(err instanceof Error) || err.message !== 'Conversation not found') throw err;
    return api.getConversation(routeSegment);
  }
}

async function getConversationMetaForRoute(routeSegment: string) {
  if (prefersConversationIdRoute(routeSegment)) {
    try {
      return await api.getConversationMeta(routeSegment);
    } catch (err) {
      if (!(err instanceof Error) || err.message !== 'Conversation not found') throw err;
      return api.getConversationMetaBySlug(routeSegment);
    }
  }
  try {
    return await api.getConversationMetaBySlug(routeSegment);
  } catch (err) {
    if (!(err instanceof Error) || err.message !== 'Conversation not found') throw err;
    return api.getConversationMeta(routeSegment);
  }
}


export function ConversationPage({ routePrefix = '/c' }: { routePrefix?: '/c' | '/global' }) {
  const { slug } = useParams<{ slug: string }>();
  const navigate = useNavigate();
  const location = useLocation();

  useEffect(() => {
    if (routePrefix === '/global' || !slug || location.hash.startsWith('#message-')) return;
    let cancelled = false;
    api.resolveCoordinatorRoute(slug)
      .then(({ coordinator_id }) => {
        if (cancelled) return;
        if (coordinator_id) {
          navigate(`/global/${coordinator_id}`, { replace: true });
        }
      })
      .catch(() => {});
    return () => { cancelled = true; };
  }, [location.hash, navigate, routePrefix, slug]);
  return (
    <ReviewNotesProvider scopeKey={slug}>
      {/* The viewer slot (prose / diff / browser) is provided by DesktopLayout,
          which wraps every conversation route. Mounted above
          ConversationPageContent's viewer early-returns so draft persistence
          survives composer unmounts. */}
      {slug && <DraftLifecycle slug={slug} />}
      <ConversationPageContent routePrefix={routePrefix} />
    </ReviewNotesProvider>
  );
}

function RecoveryBanner({ message, recoveryKind }: { message: string; recoveryKind: string }) {
  return (
    <div className="error-input-area">
      <div className="error-body">
        <div className="error-body-content">
          <div className="error-body-title">Recovery required — {recoveryKind}</div>
          <div className="error-body-details">{message}</div>
        </div>
      </div>
      <div className="error-action-bar">
        <span className="error-action-hint">This archived conversation is read-only.</span>
      </div>
    </div>
  );
}

function latestMessageSequenceId(messages: { sequence_id: number }[]): number | null {
  return messages.length > 0 ? messages[messages.length - 1]?.sequence_id ?? null : null;
}

function mergeConversationMessages<T extends { message_id: string; sequence_id: number }>(existing: T[], incoming: T[]): T[] {
  const byMessageId = new Map<string, T>();
  const bySequenceId = new Map<number, T>();

  const upsert = (message: T) => {
    const priorByMessageId = byMessageId.get(message.message_id);
    if (priorByMessageId && priorByMessageId.sequence_id <= message.sequence_id) {
      bySequenceId.delete(priorByMessageId.sequence_id);
    }

    const priorBySequenceId = bySequenceId.get(message.sequence_id);
    if (priorBySequenceId && priorBySequenceId.message_id !== message.message_id) {
      byMessageId.delete(priorBySequenceId.message_id);
    }

    if (!priorByMessageId || priorByMessageId.sequence_id <= message.sequence_id) {
      byMessageId.set(message.message_id, message);
      bySequenceId.set(message.sequence_id, message);
    }
  };

  existing.forEach(upsert);
  incoming.forEach(upsert);

  return Array.from(bySequenceId.values()).toSorted((a, b) => a.sequence_id - b.sequence_id);
}

function ConversationPageContent({ routePrefix }: { routePrefix: '/c' | '/global' }) {
  const { slug } = useParams<{ slug: string }>();
  const { setConversationReadiness } = useConversationReadiness();
  const navigate = useNavigate();
  const location = useLocation();
  const targetMessageId = useMemo(() => {
    const hash = location.hash.startsWith('#message-') ? location.hash.slice('#message-'.length) : '';
    return hash || undefined;
  }, [location.hash]);
  const createConversationWithStore = useCreateConversationWithStore();

  // Atom-backed conversation state (survives navigation via ConversationProvider).
  // `useConversationView` subscribes to only the fields the page renders from,
  // so per-token streaming churn and per-ping heartbeat bumps don't re-render
  // this component (and its non-memoized children). The streaming buffer is
  // read by <StreamingMessage>/<MessageList> via their own slice selectors;
  // the heartbeat clock by <ConnectedStateBar> via useLastSseEventAt.
  const [atom, dispatch] = useConversationView(slug!);
  const workScopeInventory = useWorkScope(slug!);

  // Derived from atom
  const conversationId = atom.conversationId ?? undefined;
  const conversation = atom.conversation;
  const [archiveStatusConfirmedConversationId, setArchiveStatusConfirmedConversationId] = useState<string | null>(null);
  const archiveStatusConfirmed =
    conversationId !== undefined && archiveStatusConfirmedConversationId === conversationId;
  const serverArchived = conversation?.archived === true;
  const cachedIsSafeOffline =
    !navigator.onLine && conversation?.archived !== true && !archiveStatusConfirmed;
  const isArchived = serverArchived || (!archiveStatusConfirmed && !cachedIsSafeOffline);
  const confirmedLive = !!conversationId && (archiveStatusConfirmed || cachedIsSafeOffline) && !serverArchived;
  const prStatusHandle = useConversationPrStatus({
    conversationId: confirmedLive ? conversationId : undefined,
    convModeLabel: conversation?.conv_mode_label,
    branchName: conversation?.branch_name,
    cachedPr: conversation?.cached_pr,
  });
  const refreshPrStatus = prStatusHandle.refresh;
  const activePrIdentity = prStatusHandle.activeSelection?.active_pr
    ? `${prStatusHandle.activeSelection.active_pr.pr.repo_owner}/${prStatusHandle.activeSelection.active_pr.pr.repo_name}#${prStatusHandle.activeSelection.active_pr.pr.pr_number}`
    : null;

  const observedWorkScopeRef = useRef<string | null>(null);
  useEffect(() => {
    if (!workScopeInventory) return;
    const previousScope = observedWorkScopeRef.current;
    observedWorkScopeRef.current = workScopeInventory.scope_key;
    if (previousScope !== workScopeInventory.scope_key || !confirmedLive) return;
    void refreshPrStatus();
  }, [workScopeInventory, confirmedLive, refreshPrStatus]);

  useEffect(() => {
    setConversationReadiness({
      conversationId: conversationId ?? null,
      confirmedLive,
    });
    return () => {
      setConversationReadiness({ conversationId: null, confirmedLive: false });
    };
  }, [setConversationReadiness, conversationId, confirmedLive]);

  // Page-level state — not conversation data
  const [error, setError] = useState<string | null>(null);
  const [deletingConversation, setDeletingConversation] = useState(false);
  const historyGenerationRef = useRef(0);
  const historyRequestTokenRef = useRef(0);
  const historyCommandTokenRef = useRef(0);
  const historyViewRef = useRef({ conversationId: '', generation: 0, transcriptGeneration: 0 });
  const [historyExpansion, dispatchHistoryExpansion] = useReducer(
    reduceHistoryExpansion,
    initialHistoryExpansionState(
      { conversationId: '', generation: 0, transcriptGeneration: 0 },
      false,
    ),
  );

  historyViewRef.current = historyExpansion.view;
  const historyCoverageRef = useRef(historyExpansion.coverage);
  historyCoverageRef.current = historyExpansion.coverage;

  // File explorer context (shared with desktop panel) — a projection of the
  // unified viewer slot below.
  const fileExplorer = useFileExplorer();
  // Unified viewer slot (specs/viewer_slot): one mutually-exclusive surface
  // (prose / diff / browser) derived from the URL. Mutual exclusion is
  // structural — opening any viewer rewrites `?viewer=` and the others close.
  // No coordinating effects: the type system enforces the single slot.
  const viewerSlot = useViewerSlot();
  const slotKind = viewerSlot.slot.kind;
  const rawDiffPresentation = viewerSlot.slot.kind === 'diff' ? viewerSlot.slot.presentation : null;
  const rawDiffTarget = viewerSlot.slot.kind === 'diff' ? viewerSlot.slot.target : 'workspace';
  const diffPresentation = isArchived ? null : rawDiffPresentation;
  const diffTarget = isArchived ? 'workspace' : rawDiffTarget;
  const fullscreenDiffOpen = diffPresentation === 'fullscreen';
  const paneDiffOpen = diffPresentation === 'pane';
  const browserOpen = slotKind === 'browser';
  // Process inspector (specs/process-inspector/, REQ-PINSP-007): a fourth slot
  // kind addressed by (scope_key, handle_id). Rendered alongside the prose /
  // diff / browser viewers in each presentation branch below.
  const inspectSlot = viewerSlot.slot.kind === 'inspect' ? viewerSlot.slot : null;
  const inspectOpen = inspectSlot !== null;
  const messageSlot = viewerSlot.slot.kind === 'message' ? viewerSlot.slot : null;
  const messageOpen = messageSlot !== null;
  const commissionReviewSlot = viewerSlot.slot.kind === 'commission-review' ? viewerSlot.slot : null;
  const commissionReviewOpen = commissionReviewSlot !== null;
  const handleCloseDiff = viewerSlot.close;
  const handleCloseBrowserView = viewerSlot.close;
  const handleCloseInspector = viewerSlot.close;
  const handleCloseMessageViewer = viewerSlot.close;
  const handleOpenBrowserView = viewerSlot.openBrowser;
  const handleOpenMessageViewer = viewerSlot.openMessage;
  // ConversationPage was previously snapshotting `isDesktop` at mount and
  // never resubscribing — a window resize across 1025px wouldn't update
  // the layout until the user navigated. The shared hooks now subscribe
  // on every consumer.
  const isDesktop = useIsDesktop();
  // Wider threshold (≥1280px) gates the split-pane prose reader (task 08654).
  // Below this we keep the existing full-screen overlay UX; above, the
  // reader sits beside the chat as a resizable sibling pane.
  const isWideDesktop = useIsWideDesktop();
  const handleOpenCommissionReview = useCallback((requestSequenceId: number) => {
    viewerSlot.openCommissionReview(requestSequenceId);
  }, [viewerSlot]);
  const VIEWER_PANE_MIN = 360;
  const VIEWER_PANE_MAX = 1200;
  const viewerPane = useResizablePane({
    key: 'viewer-pane-width',
    min: VIEWER_PANE_MIN,
    max: VIEWER_PANE_MAX,
    defaultSize: 600,
    collapseThreshold: 280,
  });

  // The viewer pane (`--viewer-pane-width`) and the terminal pane
  // (`--terminal-pane-height`, read by `TerminalPanel`) are sized from CSS
  // variables on `#app`. Each variable is owned imperatively (NOT via the React
  // `style` prop) by exactly two writers that never run concurrently: a layout
  // effect that syncs it to committed pane state, and the matching divider's
  // live-drag channel. Driving them from the style prop instead would let an
  // unrelated re-render mid-drag (streaming, heartbeat) clobber the live size
  // and snap the pane back; an effect's deps are frozen during a drag, so it
  // cannot fire until the drag commits on pointer-up. The live channel means a
  // divider drag resizes its pane without re-rendering this page (and the
  // conversation subtree below it) on every pointer move — React state catches
  // up once, on pointer-up. See `useResizablePane`'s `onLiveResize`.
  const appElementRef = useRef<HTMLDivElement | null>(null);
  const setAppCssVariable = useCallback((name: string, value: string) => {
    const appElement = appElementRef.current;
    if (appElement) appElement.style.setProperty(name, value);
  }, []);
  useLayoutEffect(() => {
    setAppCssVariable(
      '--viewer-pane-width',
      `${viewerPane.collapsed ? 0 : viewerPane.size}px`,
    );
  }, [viewerPane.collapsed, viewerPane.size, setAppCssVariable]);
  const handleViewerLiveResize = useCallback((size: number, collapsed: boolean) => {
    setAppCssVariable(
      '--viewer-pane-width',
      `${collapsed ? 0 : size}px`,
    );
  }, [setAppCssVariable]);

  // Mobile-only file browser overlay. The prose reader itself reads its
  // open-file state from `fileExplorer.openFileState` (URL-driven), so
  // mobile and desktop share a single source of truth — opening or closing
  // a file just rewrites `?file=...&root=...` on the current URL, which
  // means an iOS PWA cold reload restores the exact view.
  const [showFileBrowser, setShowFileBrowser] = useState(false);

  const sendingMessagesRef = useRef<Set<string>>(new Set());
  const inputRef = useRef<InputAreaHandle>(null);

  const { setDraftIfEmpty: setDraftIfEmptyCb, appendDraft: appendDraftCb } = useDraftActions(slug!);

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
  const { isOnline, queueOperation, removePendingOperations } = useAppMachine();

  // Toast for question panel feedback
  const { toasts, dismissToast, showInfo, showError } = useToast();

  // Attachments (not conversation state — cleared on page refresh)
  const [images, setImages] = useState<ImageData[]>([]);
  const [files, setFiles] = useState<FileAttachment[]>([]);

  // Shared models/credential poller — one request loop app-wide.
  const { models: availableModels, credentialStatus } = useModels();

  // Task approval overlay
  const [showTaskApproval, setShowTaskApproval] = useState(false);
  const [approvalContextWindowUsed, setApprovalContextWindowUsed] = useState<number | null>(null);
  const taskApprovalError = atom.uiError?.type === 'BackendError' ? atom.uiError.message : null;
  const [showFirstTaskWelcome, setShowFirstTaskWelcome] = useState(false);
  // Context-full banner: summary expanded by default; user can collapse to
  // read the conversation above.
  const [contextExhaustedExpanded, setContextExhaustedExpanded] = useState(true);

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
  // Refs (sendingMessagesRef, seedHydratedRef, cachedMessagesRef) are also
  // reset here. Mutating .current during render is safe because refs don't
  // trigger re-renders. The refs live alongside this block (vs. their
  // original declaration sites further down the file) so the reset can see
  // them and the contract "these are per-slug state" is colocated.
  const seedHydratedRef = useRef<string | null>(null);
  const cachedMessagesRef = useRef<readonly Message[]>([]);
  const cachedRowsGenerationRef = useRef<number | null>(null);
  const cacheWriteQueueRef = useRef<Promise<void>>(Promise.resolve());
  const [lastSlug, setLastSlug] = useState<string | undefined>(slug);
  if (lastSlug !== slug) {
    setLastSlug(slug);
    // useState resets — React batches these into the same render.
    setError(null);
    setDeletingConversation(false);
    setShowFileBrowser(false);
    setImages([]);
    setFiles([]);
    setShowTaskApproval(false);
    setApprovalContextWindowUsed(null);
    setShowFirstTaskWelcome(false);
    setContextExhaustedExpanded(true);
    setFocusToken(0);
    // Ref resets — immediate, no re-render.
    sendingMessagesRef.current = new Set();
    seedHydratedRef.current = null;
    cachedMessagesRef.current = [];
    cachedRowsGenerationRef.current = null;
    cacheWriteQueueRef.current = Promise.resolve();
  }
  // Terminal split-pane height — collapses to a 32px header strip.
  // Default collapsed: most conversations don't use the terminal, and an
  // expanded default eats vertical space + spins up the WebSocket/xterm.
  const terminalPane = useResizablePane({
    key: terminalPaneStorageKey(slug!),
    min: TERMINAL_COLLAPSED_PX,
    max: terminalPaneMax,
    defaultSize: 300,
    collapseThreshold: 60,
    defaultCollapsed: true,
  });

  // Terminal-pane height live-drag channel — mirrors the viewer pane above.
  // `TerminalPanel` reads `--terminal-pane-height` (falling back to its `height`
  // prop); the layout effect syncs it to committed state and the divider's
  // `onLiveResize` drives it during a drag, so resizing the terminal does not
  // re-render this page. The xterm fit is driven by a ResizeObserver on the
  // panel element, so it keeps tracking the live height without React.
  useLayoutEffect(() => {
    setAppCssVariable(
      '--terminal-pane-height',
      `${terminalPane.collapsed ? TERMINAL_COLLAPSED_PX : terminalPane.size}px`,
    );
  }, [terminalPane.collapsed, terminalPane.size, setAppCssVariable]);
  const handleTerminalLiveResize = useCallback((size: number, collapsed: boolean) => {
    setAppCssVariable(
      '--terminal-pane-height',
      `${collapsed ? TERMINAL_COLLAPSED_PX : size}px`,
    );
  }, [setAppCssVariable]);

  // Callback ref for `#app`. The layout effects above only re-run on pane-state
  // changes; the conversation-load path first paints a skeleton `#app` (without
  // this ref) and mounts the real host later *without* a pane-state change, so
  // those effects would never seed the variables on the real element. Syncing on
  // attach (from the latest committed values, held in a ref to keep the callback
  // stable) guarantees the host opens at the stored width, not the CSS fallback.
  const paneVarsRef = useRef({ viewerWidth: '', terminalHeight: '' });
  paneVarsRef.current = {
    viewerWidth: `${viewerPane.collapsed ? 0 : viewerPane.size}px`,
    terminalHeight: `${terminalPane.collapsed ? TERMINAL_COLLAPSED_PX : terminalPane.size}px`,
  };
  const setAppElement = useCallback((el: HTMLDivElement | null) => {
    appElementRef.current = el;
    if (el) {
      el.style.setProperty('--viewer-pane-width', paneVarsRef.current.viewerWidth);
      el.style.setProperty('--terminal-pane-height', paneVarsRef.current.terminalHeight);
    }
  }, []);

  // Credential helper auto-open — shared hook consolidates the pattern.
  const { showAuthPanel, setShowAuthPanel } = useAutoAuth(credentialStatus);

  const eventCursorRef = useConversationEventCursorRef(slug!);

  // Message queue management. `queuedMessages` is the raw store; the rendered
  // split between "pending in the message list" and "failed in the input area"
  // is derived below.
  const {
    queuedMessages,
    enqueue,
    markFailed,
    markAccepted,
    markSteeringQueued,
    markRecoverableInconsistency,
    reconcileAuthoritative,
    dismiss,
  } = useMessageQueue(conversationId);

  // Pending messages shown in the conversation are a pure derivation of the
  // queue and `atom.messages` — see `derivePendingMessages` for the rule.
  const pendingMessages = useMemo(
    () => derivePendingMessages(queuedMessages, atom.messages.map((m) => m.message_id)),
    [atom.messages, queuedMessages],
  );

  const viewableMessages = atom.messages;

  // Failed messages are rendered in InputArea with retry/dismiss controls.
  const failedMessages = useMemo(
    () => deriveFailedMessages(queuedMessages),
    [queuedMessages],
  );

  const atomRef = useRef(atom);
  atomRef.current = atom;

  useEffect(() => {
    if (!conversationId || atom.transcriptGeneration === null) return;
    const currentView = historyViewRef.current;
    if (
      currentView.conversationId === conversationId
      && currentView.transcriptGeneration === atom.transcriptGeneration
    ) return;
    historyGenerationRef.current += 1;
    dispatchHistoryExpansion({
      type: 'view_changed',
      view: {
        conversationId,
        generation: historyGenerationRef.current,
        transcriptGeneration: atom.transcriptGeneration,
      },
      hasEarlierHistory: atom.transcriptCoverage === 'tail',
    });
  }, [conversationId, atom.transcriptGeneration, atom.transcriptCoverage]);

  const connectionInfo = useConnection({
    conversationId,
    dispatch,
    getLastAppliedEventSeq: () => eventCursorRef.current,
    getInitialRequestMode: () => {
      const latestLoadedMessageSeq = latestMessageSequenceId(atomRef.current.messages);
      if (latestLoadedMessageSeq === null) return { kind: 'full' };
      const transcriptGeneration = atomRef.current.transcriptGeneration;
      if (transcriptGeneration === null) return { kind: 'full' };
      return { kind: 'messages_after_floor', afterMessageFloor: latestLoadedMessageSeq, transcriptGeneration };
    },
  });

  const isOffline =
    connectionInfo.state === 'offline' || connectionInfo.state === 'reconnecting';
  const isConnected =
    connectionInfo.state === 'connected' || connectionInfo.state === 'reconnected';

  // Authoritative echoes are terminal for local queue ownership. Compact them
  // from localStorage instead of merely filtering them from this render.
  useEffect(() => {
    reconcileAuthoritative(atom.messages.map((message) => message.message_id));
  }, [atom.messages, reconcileAuthoritative]);

  const idleReconciliationKeyRef = useRef<string | null>(null);
  const optimisticPhaseOwnerRef = useRef<string | null>(null);
  useEffect(() => {
    if (atom.phase.type !== 'awaiting_llm') {
      optimisticPhaseOwnerRef.current = null;
    }
  }, [atom.phase.type, atom.phaseLastAppliedEventSeq]);
  useEffect(() => {
    if (!conversationId || !isConnected || atom.phase.type !== 'idle') return;
    const accepted = queuedMessages.filter((message) => {
      if (message.status === 'accepted') return true;
      return message.status === 'steering_queued'
        && atom.phaseLastAppliedEventSeq > (message.acceptedAfterEventSeq ?? 0);
    });
    if (accepted.length === 0) return;

    const key = [
      conversationId,
      atom.phaseLastAppliedEventSeq,
      accepted.map((message) => message.localId).join(','),
    ].join(':');
    if (idleReconciliationKeyRef.current === key) return;
    idleReconciliationKeyRef.current = key;

    let cancelled = false;
    void (async () => {
      try {
        const snapshotStartedAtEventSeq = eventCursorRef.current;
        const settledResults = await Promise.allSettled(
          Array.from({ length: Math.ceil(accepted.length / 100) }, (_, index) => {
            const chunk = accepted.slice(index * 100, (index + 1) * 100);
            return api.reconcileAcceptedMessages(
              conversationId,
              chunk.map((message) => message.localId),
            );
          }),
        );
        if (cancelled) return;

        const results = settledResults.flatMap((result) =>
          result.status === 'fulfilled' ? [result.value] : []
        );
        if (results.length === 0) {
          throw new Error('All accepted-message reconciliation chunks failed');
        }
        if (results.length < settledResults.length) {
          idleReconciliationKeyRef.current = null;
          console.warn('[message-queue] some reconciliation chunks failed', {
            conversationId,
            failedChunks: settledResults.length - results.length,
          });
        }
        const entries = results.flatMap((result) => result.entries);
        const persisted = entries.filter((entry) => entry.status === 'persisted');
        const current = atomRef.current;
        if (
          persisted.length > 0
          && current.conversationId === conversationId
          && current.conversation
        ) {
          dispatch({
            type: 'merge_conversation_data',
            conversationId,
            conversation: current.conversation,
            messages: persisted.map((entry) => entry.message),
            phase: current.phase,
            contextWindow: current.contextWindow,
            ...(current.transcriptGeneration !== null && {
              transcriptGeneration: current.transcriptGeneration,
            }),
            transcriptCoverage: current.transcriptCoverage,
            snapshotStartedAtEventSeq,
          });
          reconcileAuthoritative(persisted.map((entry) => entry.message_id));
        }

        if (results.every((result) => result.conversation_idle)) {
          for (const entry of entries) {
            if (entry.status === 'absent') {
              markRecoverableInconsistency(entry.message_id);
            }
          }
        } else {
          idleReconciliationKeyRef.current = null;
        }
      } catch (error) {
        idleReconciliationKeyRef.current = null;
        console.warn('[message-queue] authoritative idle reconciliation failed', {
          conversationId,
          messageIds: accepted.map((message) => message.localId),
          error,
        });
      }
    })();

    return () => {
      cancelled = true;
      if (idleReconciliationKeyRef.current === key) {
        idleReconciliationKeyRef.current = null;
      }
    };
  }, [
    conversationId,
    isConnected,
    atom.phase.type,
    atom.phaseLastAppliedEventSeq,
    queuedMessages,
    markRecoverableInconsistency,
    reconcileAuthoritative,
    dispatch,
    eventCursorRef,
  ]);


  // Load conversation by slug — skip if atom already has data from a previous visit
  useEffect(() => {
    if (!slug) {
      navigate('/');
      return;
    }

    setError(null);
    setArchiveStatusConfirmedConversationId(null);
    historyGenerationRef.current += 1;
    dispatchHistoryExpansion({
      type: 'view_changed',
      view: {
        conversationId: atomRef.current.conversationId ?? slug,
        generation: historyGenerationRef.current,
        transcriptGeneration: atomRef.current.transcriptGeneration ?? 0,
      },
      hasEarlierHistory: false,
    });

    const hadAtomData = !!atomRef.current.conversationId;

    let cancelled = false;

    const loadConversation = async () => {
      try {
        let cached = atomRef.current.conversation;
        let cachedMessages: Message[] = hadAtomData ? atomRef.current.messages : [];
        if (!hadAtomData) {
          cached = await getCachedConversationForRoute(slug);
          if (cached) {
            cachedMessages = await cacheDB.getMessages(cached.id);
            if (!cancelled) {
              dispatch({
                type: 'set_initial_data',
                conversationId: cached.id,
                conversation: cached,
                messages: cachedMessages,
                phase: cached.state ? parseConversationState(cached.state) : { type: 'idle' },
                contextWindow: { used: 0 },
                transcriptGeneration: cached.transcript_generation ?? 1,
                transcriptCoverage: 'tail',
                eventCursorFloor: eventCursorRef.current,
              });
            }
          }
        }

        // Step 2: Fetch authoritative data from network
        if (navigator.onLine && !cancelled) {
          const cachedConversationId = cached?.id ?? null;
          const hasCachedMessages = cachedConversationId !== null && cachedMessages.length > 0;
          const cachedReplicaMeta = cachedConversationId
            ? await cacheDB.getReplicaMeta(cachedConversationId)
            : null;
          let metadata = await getConversationMetaForRoute(slug);
          if (cancelled) return;
          let metadataTranscriptGeneration = metadata.conversation.transcript_generation ?? 1;
          const cachedRowsTranscriptGeneration = cachedReplicaMeta?.transcriptGeneration ?? null;
          const cacheGenerationMatchesMetadata = cachedRowsTranscriptGeneration !== null
            && cachedRowsTranscriptGeneration === metadataTranscriptGeneration;

          if (hasCachedMessages && cachedConversationId && cacheGenerationMatchesMetadata) {
            try {
              const snapshotStartedAtEventSeq = eventCursorRef.current;
              const mergedTranscriptTail = latestMessageSequenceId(cachedMessages);
              if (mergedTranscriptTail !== null) {
                let contiguousTranscriptTail = mergedTranscriptTail;
                let mergedMessages = cachedMessages;
                let latestServerTail: number | null = null;
                let latestTranscriptGeneration = cachedRowsTranscriptGeneration;

                while (!cancelled) {
                  const catchUp = await api.getConversationMessagesAfter(cachedConversationId, contiguousTranscriptTail, 200);
                  latestServerTail = catchUp.server_message_tail;
                  latestTranscriptGeneration = catchUp.transcript_generation;
                  if (catchUp.messages.length > 0) {
                    mergedMessages = mergeConversationMessages(mergedMessages, catchUp.messages);
                    await cacheDB.putMessages(catchUp.messages);
                  }
                  if (catchUp.messages.length > 0) {
                    const nextContiguousTail = catchUp.messages.at(-1)!.sequence_id;
                    if (nextContiguousTail <= contiguousTranscriptTail) break;
                    contiguousTranscriptTail = nextContiguousTail;
                  }
                  if (
                    catchUp.messages.length === 0 ||
                    latestServerTail === null ||
                    contiguousTranscriptTail >= latestServerTail
                  ) {
                    break;
                  }
                }
                if (cancelled) return;

                const latestWindow = await api.getConversationMessagesLatest(cachedConversationId, 50);
                latestServerTail = latestWindow.server_message_tail;
                latestTranscriptGeneration = latestWindow.transcript_generation;
                if (latestWindow.messages.length > 0) {
                  mergedMessages = mergeConversationMessages(mergedMessages, latestWindow.messages);
                  await cacheDB.putMessages(latestWindow.messages);
                }
                if (cancelled) return;

                while (!cancelled && latestServerTail !== null && contiguousTranscriptTail < latestServerTail) {
                  const catchUp = await api.getConversationMessagesAfter(
                    cachedConversationId,
                    contiguousTranscriptTail,
                    200,
                  );
                  latestServerTail = catchUp.server_message_tail;
                  latestTranscriptGeneration = catchUp.transcript_generation;
                  if (catchUp.messages.length === 0) break;
                  const nextContiguousTail = catchUp.messages.at(-1)!.sequence_id;
                  if (nextContiguousTail <= contiguousTranscriptTail) break;
                  mergedMessages = mergeConversationMessages(mergedMessages, catchUp.messages);
                  await cacheDB.putMessages(catchUp.messages);
                  contiguousTranscriptTail = nextContiguousTail;
                }
                if (cancelled) return;

                await cacheDB.putReplicaMeta({
                  conversationId: cachedConversationId,
                  latestMessageSequenceId: contiguousTranscriptTail,
                  latestEventSequenceId: null,
                  transcriptGeneration: latestTranscriptGeneration,
                  lastHydratedAt: new Date().toISOString(),
                });

                const authoritativeConversation = metadata.conversation;
                if (authoritativeConversation.id !== cachedConversationId) {
                  throw new Error('Cached conversation no longer owns the requested slug');
                }
                setArchiveStatusConfirmedConversationId(authoritativeConversation.id);
                await cacheDB.putConversation(authoritativeConversation);

                if (
                  atomRef.current.conversationId === null
                  || atomRef.current.conversationId === authoritativeConversation.id
                ) {
                  dispatchHistoryExpansion({
                    type: 'view_changed',
                    view: {
                      conversationId: authoritativeConversation.id,
                      generation: historyGenerationRef.current,
                      transcriptGeneration: latestTranscriptGeneration,
                    },
                    hasEarlierHistory: latestWindow.has_older_messages,
                  });
                  dispatch({
                    type: 'merge_conversation_data',
                    conversationId: authoritativeConversation.id,
                    conversation: authoritativeConversation,
                    messages: mergedMessages,
                    phase: authoritativeConversation.state
                      ? parseConversationState(authoritativeConversation.state)
                      : { type: 'idle' },
                    contextWindow: { used: metadata.context_window_size || 0 },
                    transcriptGeneration: latestTranscriptGeneration,
                    transcriptCoverage: latestWindow.has_older_messages ? 'tail' : 'complete',
                    eventCursorFloor: snapshotStartedAtEventSeq,
                    snapshotStartedAtEventSeq,
                  });
                }
                return;
              }
            } catch (err) {
              if (!cancelled) {
                console.warn('Incremental conversation catch-up failed; falling back to full fetch:', err);
              }
            }
          }

          try {
            const snapshotStartedAtEventSeq = eventCursorRef.current;
            let latestWindow;
            for (let attempt = 0; attempt < 3; attempt += 1) {
              try {
                latestWindow = await api.getConversationMessagesLatest(metadata.conversation.id, 50);
              } catch (error) {
                if (!(error instanceof MessageSliceAlignmentError)) throw error;
                const full = await api.getConversation(metadata.conversation.id);
                latestWindow = {
                  messages: full.messages,
                  has_older_messages: false,
                  server_message_tail: latestMessageSequenceId(full.messages),
                  transcript_generation: full.conversation.transcript_generation ?? metadata.conversation.transcript_generation ?? 1,
                };
              }
              if (metadataTranscriptGeneration === latestWindow.transcript_generation) break;
              if (attempt === 2) throw new Error('Conversation transcript kept changing while loading');
              metadata = await getConversationMetaForRoute(slug);
              metadataTranscriptGeneration = metadata.conversation.transcript_generation ?? 1;
            }
            if (!latestWindow) throw new Error('Failed to load conversation messages');
            const result = { ...metadata, messages: latestWindow.messages };
            if (!cancelled) {
              dispatchHistoryExpansion({
                type: 'view_changed',
                view: {
                  conversationId: result.conversation.id,
                  generation: historyGenerationRef.current,
                  transcriptGeneration: metadataTranscriptGeneration,
                },
                hasEarlierHistory: latestWindow.has_older_messages,
              });
              const replacesDifferentConversation = atomRef.current.conversationId !== null
                && atomRef.current.conversationId !== result.conversation.id;
              dispatch({
                type: eventCursorRef.current > 0 && !replacesDifferentConversation ? 'merge_conversation_data' : 'set_initial_data',
                reset: replacesDifferentConversation,
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
                transcriptGeneration: metadataTranscriptGeneration,
                transcriptCoverage: latestWindow.has_older_messages ? 'tail' : 'complete',
                eventCursorFloor: snapshotStartedAtEventSeq,
                snapshotStartedAtEventSeq,
              });
              setArchiveStatusConfirmedConversationId(result.conversation.id);
              await cacheDB.putConversation(result.conversation);
              await cacheDB.putMessages(result.messages);
              await cacheDB.putReplicaMeta({
                conversationId: result.conversation.id,
                latestMessageSequenceId: latestMessageSequenceId(result.messages),
                latestEventSequenceId: null,
                transcriptGeneration: metadataTranscriptGeneration,
                lastHydratedAt: new Date().toISOString(),
              });
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
  }, [slug, navigate, dispatch, eventCursorRef]);

  const loadOlderMessagesForIntent = useCallback(async (intent: HistoryIntent) => {
    if (!slug || !conversationId || historyExpansion.coverage !== 'tail' || historyExpansion.activeRequest) return;
    const request = {
      token: ++historyRequestTokenRef.current,
      view: historyExpansion.view,
      snapshotStartedAtEventSeq: eventCursorRef.current,
      intent,
    };
    const requestTranscriptGeneration = request.view.transcriptGeneration;

    dispatchHistoryExpansion({ type: 'request_started', request });
    try {
      const result = await getConversationForRoute(slug);
      const currentView = historyViewRef.current;
      const authoritativeTranscriptGeneration = atomRef.current.transcriptGeneration;
      const responseTranscriptGeneration = result.conversation.transcript_generation ?? 1;
      const requestIsCurrent = result.conversation.id === request.view.conversationId
        && currentView.conversationId === request.view.conversationId
        && currentView.generation === request.view.generation
        && currentView.transcriptGeneration === request.view.transcriptGeneration
        && authoritativeTranscriptGeneration === request.view.transcriptGeneration
        && responseTranscriptGeneration === request.view.transcriptGeneration;
      if (!requestIsCurrent) {
        dispatchHistoryExpansion({
          type: 'history_failed',
          requestToken: request.token,
          view: request.view,
          transcriptGeneration: requestTranscriptGeneration,
          message: 'Conversation changed while loading earlier history',
        });
        return;
      }
      dispatch({
        type: 'merge_conversation_data',
        conversationId: request.view.conversationId,
        conversation: result.conversation,
        messages: result.messages,
        phase: result.conversation.state
          ? parseConversationState(result.conversation.state)
          : result.presentation_mode === 'working'
            ? { type: 'awaiting_llm' }
            : { type: 'idle' },
        contextWindow: { used: result.context_window_size || 0 },
        transcriptGeneration: responseTranscriptGeneration,
        transcriptCoverage: 'complete',
        eventCursorFloor: historyMergeEventCursorFloor(request),
        snapshotStartedAtEventSeq: request.snapshotStartedAtEventSeq,
      });
      dispatchHistoryExpansion({
        type: 'history_loaded',
        requestToken: request.token,
        view: request.view,
        targetPresent: request.intent.kind !== 'deep_link'
          || findHistoricalUnitIndexByMessageId(
            buildHistoricalUnits({ messages: result.messages, pendingMessages: [] }).historicalUnits,
            request.intent.targetMessageId,
          ) >= 0,
        commandToken: ++historyCommandTokenRef.current,
      });
      await cacheDB.putMessages(result.messages);
    } catch (err) {
      console.warn('Failed to load earlier conversation history:', err);
      dispatchHistoryExpansion({
        type: 'history_failed',
        requestToken: request.token,
        view: request.view,
        transcriptGeneration: requestTranscriptGeneration,
        message: err instanceof Error ? err.message : 'Failed to load earlier history',
      });
    }
  }, [slug, conversationId, historyExpansion, dispatch, eventCursorRef]);

  const loadOlderMessages = useCallback((restoreBasis?: RestoreBasis) => {
    void loadOlderMessagesForIntent({
      kind: 'reader_expansion',
      restore: restoreBasis ?? { kind: 'following_tail' },
    });
  }, [loadOlderMessagesForIntent]);

  const updateOlderMessagesRestore = useCallback((restore: RestoreBasis) => {
    const active = historyExpansion.activeRequest;
    if (!active || active.intent.kind !== 'reader_expansion') return;
    dispatchHistoryExpansion({
      type: 'reader_restore_updated',
      requestToken: active.token,
      view: active.view,
      restore,
    });
  }, [historyExpansion.activeRequest]);

  useEffect(() => {
    dispatchHistoryExpansion({ type: 'target_changed', targetMessageId: targetMessageId ?? null });
  }, [targetMessageId]);

  const requestedLoadedTargetRef = useRef<string | null>(null);
  const loadedHistoricalUnits = useMemo(
    () => buildHistoricalUnits({ messages: atom.messages, pendingMessages }).historicalUnits,
    [atom.messages, pendingMessages],
  );
  const loadedTargetPresent = targetMessageId
    ? findHistoricalUnitIndexByMessageId(loadedHistoricalUnits, targetMessageId) >= 0
    : false;
  const loadedTargetRequestKey = targetMessageId
    ? `${historyExpansion.view.conversationId}:${historyExpansion.view.generation}:${historyExpansion.view.transcriptGeneration}:${targetMessageId}`
    : null;
  useEffect(() => {
    if (!loadedTargetRequestKey) {
      requestedLoadedTargetRef.current = null;
      return;
    }
    if (
      (historyExpansion.coverage === 'complete' || loadedTargetPresent)
      && !historyExpansion.pendingCommand
      && !historyExpansion.failure
      && requestedLoadedTargetRef.current !== loadedTargetRequestKey
    ) {
      requestedLoadedTargetRef.current = loadedTargetRequestKey;
      dispatchHistoryExpansion({
        type: 'loaded_target_requested',
        targetMessageId: targetMessageId!,
        commandToken: ++historyCommandTokenRef.current,
      });
    }
  }, [targetMessageId, loadedTargetPresent, loadedTargetRequestKey, historyExpansion.coverage, historyExpansion.pendingCommand, historyExpansion.failure]);

  useEffect(() => {
    if (targetMessageId && !loadedTargetPresent && historyExpansion.coverage === 'tail' && !historyExpansion.activeRequest && !historyExpansion.failure) {
      void loadOlderMessagesForIntent({ kind: 'deep_link', targetMessageId });
    }
  }, [targetMessageId, loadedTargetPresent, historyExpansion, loadOlderMessagesForIntent]);

  const handleHistoryScrollCommand = useCallback((token: number, result: 'applied' | 'target_missing' | 'superseded', view: HistoryScrollCommand['view']) => {
    dispatchHistoryExpansion({
      type: 'command_acknowledged',
      commandToken: token,
      view,
      result,
    });
  }, []);

  useEffect(() => {
    if (!slug || !conversationId || archiveStatusConfirmed || !isConnected) return;
    let cancelled = false;

    const confirmArchiveStatus = async () => {
      const snapshotStartedAtEventSeq = eventCursorRef.current;
      try {
        const result = await getConversationMetaForRoute(slug);
        if (cancelled || result.conversation.id !== atomRef.current.conversationId) return;
        dispatch({
          type: 'merge_conversation_data',
          conversationId: result.conversation.id,
          conversation: result.conversation,
          messages: [],
          phase: result.conversation.state
            ? parseConversationState(result.conversation.state)
            : result.presentation_mode === 'working'
              ? { type: 'awaiting_llm' }
              : { type: 'idle' },
          contextWindow: { used: result.context_window_size || 0 },
          transcriptGeneration: atomRef.current.transcriptGeneration ?? result.conversation.transcript_generation ?? 1,
          eventCursorFloor: snapshotStartedAtEventSeq,
          snapshotStartedAtEventSeq,
        });
        setArchiveStatusConfirmedConversationId(result.conversation.id);
        await cacheDB.putConversation(result.conversation);
      } catch (err) {
        if (!cancelled) console.warn('Failed to confirm archive status:', err);
      }
    };

    void confirmArchiveStatus();
    return () => {
      cancelled = true;
    };
  }, [slug, conversationId, archiveStatusConfirmed, isConnected, dispatch, eventCursorRef]);

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
  // re-hydrate it. Dispatches through `setDraftIfEmptyCb` into `DraftStore`;
  // `<DraftLifecycle>` mirrors the value to `phoenix:draft:<id>` after that.
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
    setDraftIfEmptyCb(seed);
    requestComposerFocus();
    try {
      localStorage.removeItem(key);
    } catch {
      // ignore
    }
  }, [conversationId, setDraftIfEmptyCb, requestComposerFocus]);

  // Auto-open/close task approval overlay on state transitions
  useEffect(() => {
    if (atom.phase.type === 'awaiting_task_approval' && !isArchived) {
      setShowTaskApproval(true);
    } else {
      setShowTaskApproval(false);
      setApprovalContextWindowUsed(null);
    }
  }, [atom.phase.type, isArchived]);

  useEffect(() => {
    if (!showTaskApproval || atom.phase.type !== 'awaiting_task_approval' || !conversationId) {
      setApprovalContextWindowUsed(null);
      return;
    }

    let cancelled = false;
    setApprovalContextWindowUsed(null);
    api.getConversation(conversationId)
      .then((result) => {
        if (!cancelled) setApprovalContextWindowUsed(result.context_window_size);
      })
      .catch(() => {
        if (!cancelled) setApprovalContextWindowUsed(null);
      });

    return () => {
      cancelled = true;
    };
  }, [showTaskApproval, atom.phase.type, conversationId, atom.contextWindow.used]);

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

  useEffect(() => {
    if (!atom.conversationId || atom.transcriptGeneration === null) return;
    const generationChanged = cachedRowsGenerationRef.current !== atom.transcriptGeneration;
    const cacheWrite = messageCacheWrite(
      atom.conversationId,
      cachedMessagesRef.current,
      atom.messages,
      generationChanged,
    );
    cachedMessagesRef.current = atom.messages;
    cachedRowsGenerationRef.current = atom.transcriptGeneration;
    if (cacheWrite.kind === 'append' && cacheWrite.messages.length === 0) return;
    const conversationId = atom.conversationId;
    const transcriptGeneration = atom.transcriptGeneration;
    const latestSequenceId = latestMessageSequenceId(atom.messages);
    cacheWriteQueueRef.current = cacheWriteQueueRef.current
      .catch(() => undefined)
      .then(async () => {
        if (cacheWrite.kind === 'replace') {
          await cacheDB.replaceMessages(cacheWrite.conversationId, cacheWrite.messages);
        } else {
          await cacheDB.putMessages(cacheWrite.messages);
        }
        await cacheDB.putReplicaMeta({
          conversationId,
          latestMessageSequenceId: latestSequenceId,
          latestEventSequenceId: null,
          transcriptGeneration,
          lastHydratedAt: new Date().toISOString(),
        });
      });
  }, [atom.conversationId, atom.messages, atom.transcriptGeneration]);

  // Cache conversation metadata when it changes
  useEffect(() => {
    if (atom.conversation) {
      void cacheDB.putConversation(atom.conversation);
    }
  }, [atom.conversation]);

  // REQ-BT-018 browser-session edges (rising-edge auto-open, falling-edge
  // auto-close) live in ViewerSlotProvider now — the slot owns the
  // browser_session_active flag and the single-slot mutex, so the edge handling
  // is no longer duplicated here.

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
      imgs: { data: string; media_type: string }[] = [],
      files: FileAttachment[] = []
    ) => {
      if (!conversationId) return;
      if (isArchived) return;

      sendingMessagesRef.current.add(localId);

      const phaseEventSeqBeforePost = atomRef.current.phaseLastAppliedEventSeq;
      const phaseBeforePost = atomRef.current.phase;
      const optimisticPhaseOwner = (
        (phaseBeforePost.type === 'idle' || phaseBeforePost.type === 'error')
        && optimisticPhaseOwnerRef.current === null
      ) ? localId : null;
      if (optimisticPhaseOwner) {
        optimisticPhaseOwnerRef.current = optimisticPhaseOwner;
      }
      const rollbackOptimisticPhase = () => {
        if (
          optimisticPhaseOwner
          && optimisticPhaseOwnerRef.current === optimisticPhaseOwner
          && atomRef.current.phase.type === 'awaiting_llm'
          && atomRef.current.phaseLastAppliedEventSeq === phaseEventSeqBeforePost
        ) {
          dispatch({
            type: 'local_phase_change',
            phase: phaseBeforePost,
            expectedConversationId: conversationId,
          });
          optimisticPhaseOwnerRef.current = null;
        }
      };

      try {
        if (isOnline) {
          if (optimisticPhaseOwner) {
            dispatch({
              type: 'local_phase_change',
              phase: { type: 'awaiting_llm' },
              expectedConversationId: conversationId,
            });
          }
          const result = await api.sendMessage(conversationId, text, imgs, files, localId);
          // Don't touch the queue here. The entry stays `pending` until
          // `atom.messages` contains a row with `message_id == localId`
          // (SSE echo), at which point `pendingMessages` filters it out
          // via the derivation above.
          //
          if (result.steering) {
            // Conversation was busy — message queued server-side for delivery
            // when the conversation next reaches Idle. Show a "Queued" pill
            // on the message bubble instead of the normal sending spinner.
            markSteeringQueued(localId, phaseEventSeqBeforePost);
            rollbackOptimisticPhase();
          } else {
            markAccepted(localId, phaseEventSeqBeforePost);
            if (result.already_persisted) {
              rollbackOptimisticPhase();
            }
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
            payload: { text, images: imgs, files, localId },
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
          rollbackOptimisticPhase();
          // Re-throw so InputArea can display inline error (REQ-IR-007)
          throw err;
        }
        console.error('Failed to send message:', err);
        markFailedRef.current(localId);
        rollbackOptimisticPhase();
      } finally {
        if (
          optimisticPhaseOwner
          && optimisticPhaseOwnerRef.current === optimisticPhaseOwner
          && atomRef.current.phase.type !== 'awaiting_llm'
        ) {
          optimisticPhaseOwnerRef.current = null;
        }
        sendingMessagesRef.current.delete(localId);
      }
    },
    [conversationId, isArchived, isOnline, queueOperation, dispatch, markAccepted, markSteeringQueued]
  );

  const sendMessageRef = useRef(sendMessage);
  useEffect(() => { sendMessageRef.current = sendMessage; }, [sendMessage]);

  useEffect(() => {
    if (!serverArchived) return;
    for (const msg of pendingMessages) {
      dismiss(msg.localId);
    }
    if (conversationId) {
      void removePendingOperations(conversationId, 'send_message').catch((err) => {
        console.error('Failed to drop archived pending operations:', err);
      });
    }
  }, [serverArchived, conversationId, pendingMessages, dismiss, removePendingOperations]);

  // Send queued messages when connection is restored. Iterate the derived
  // `pendingMessages` (NOT raw `queuedMessages`) so we don't re-POST entries
  // the server already has — those were filtered out by the derivation.
  // Skip `steering_queued` messages — they are already held server-side and
  // will be delivered automatically when the conversation reaches Idle.
  useEffect(() => {
    if (!isConnected || !conversationId || isArchived) return;

    for (const msg of pendingMessages) {
      if (msg.status === 'accepted' || msg.status === 'steering_queued') continue;
      if (sendingMessagesRef.current.has(msg.localId)) continue;
      sendMessageRef.current(msg.localId, msg.text, msg.images, msg.files ?? []);
    }
  }, [isConnected, conversationId, isArchived, pendingMessages]);

  const handleSend = useCallback(async (text: string, attachedImages: ImageData[], attachedFiles: FileAttachment[] = []) => {
    if (!conversationId || isArchived) return;

    const msg = enqueue(text, attachedImages, attachedFiles);

    if (isConnected) {
      // Await so expansion errors propagate back to InputArea (REQ-IR-007)
      await sendMessage(msg.localId, text, attachedImages, attachedFiles);
    }
  }, [conversationId, isArchived, enqueue, isConnected, sendMessage]);

  const handleRetry = useCallback((localId: string) => {
    const msg = queuedMessages.find((m) => m.localId === localId);
    if (!msg) return;

    // Populate the message back into the input area for review/editing
    // instead of directly resending (the banner truncates content and
    // the user may want to fix the issue that caused the failure).
    dismiss(localId);
    appendDraftCb(msg.text);
    setFiles(msg.files ?? []);
    requestComposerFocus();
  }, [queuedMessages, dismiss, appendDraftCb, setFiles, requestComposerFocus]);

  const handleCancel = useCallback(async () => {
    if (!conversationId || !canCancelConversationState(atom.phase)) return;
    if (isCancellingState(atom.phase)) return;

    try {
      await api.cancelConversation(conversationId);
    } catch (err) {
      console.error('Failed to cancel:', err);
    }
  }, [conversationId, atom.phase]);

  const handleCancelSteering = useCallback(async (localId: string) => {
    if (!conversationId) return;
    try {
      await api.cancelSteeringMessage(conversationId, localId);
      dismiss(localId);
    } catch (err) {
      console.error('Failed to cancel steering message:', err);
    }
  }, [conversationId, dismiss]);

  // Fork proposal review outcomes (REQ-PROJ-034 / 037): navigate to the new
  // fork / refinement conversation, or toast a terminal/conflict result.
  const handleForkOutcome = useCallback(
    (outcome: ForkActionOutcome) => {
      switch (outcome.kind) {
        case 'spawned':
        case 'promoted':
          if (outcome.conversationId) {
            const label = outcome.kind === 'spawned' ? 'fork' : 'refinement';
            api
              .getConversationSlug(outcome.conversationId)
              .then((s) => {
                if (s) navigate(`/c/${s}`);
                else showInfo(`Created ${label} conversation.`);
              })
              .catch(() => showInfo(`Created ${label} conversation.`));
          }
          break;
        case 'dismissed':
          showInfo('Proposal dismissed.');
          break;
        case 'already_resolved':
          showInfo('This proposal was already resolved.');
          break;
        default:
          outcome.kind satisfies never;
      }
    },
    [navigate, showInfo],
  );

  const handleTriggerContinuation = useCallback(async () => {
    if (!conversationId || isArchived) return;

    try {
      await api.triggerContinuation(conversationId);
      dispatch({
        type: 'local_phase_change',
        phase: { type: 'awaiting_continuation', attempt: 1 },
        expectedConversationId: conversationId,
      });
    } catch (err) {
      console.error('Failed to trigger continuation:', err);
    }
  }, [conversationId, isArchived, dispatch]);

  const handleUpgradeModel = useCallback(async (newModelId: string) => {
    if (!conversationId || isArchived || !canChangeModelInState(atom.phase)) return;

    try {
      await api.upgradeModel(conversationId, newModelId);
      showInfo(`Switched to ${newModelId}`);
      dispatch({ type: 'local_conversation_update', updates: { model: newModelId }, expectedConversationId: conversationId });
    } catch (err) {
      console.error('Failed to upgrade model:', err);
    }
  }, [conversationId, isArchived, atom.phase, showInfo, dispatch]);

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
      const messageId = generateUUID();
      const clientConversationId = generateUUID();
      try {
        localStorage.setItem(`seed-draft:${clientConversationId}`, promptText);
      } catch {
        // ignore — non-fatal
      }
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
        [],
        null,
        clientConversationId,
      );
      navigate(routeForConversation(newConv));
    },
    [conversation, navigate, createConversationWithStore],
  );

  const handleApproveTask = async (handoff: 'continue_in_current_conversation' | 'start_fresh_work_conversation') => {
    if (!conversationId || isArchived) return;
    dispatch({ type: 'clear_error' });
    try {
      const result = await api.approveTask(conversationId, handoff);
      if (result.first_task) {
        setShowFirstTaskWelcome(true);
      }
    } catch (err) {
      console.error('Failed to approve task:', err);
    }
  };

  const handleRejectTask = async () => {
    if (!conversationId || isArchived) return;
    try {
      await api.rejectTask(conversationId);
    } catch (err) {
      console.error('Failed to reject task:', err);
    }
  };

  const handleApproveCommissionReview = async () => {
    if (!conversationId) return;
    try {
      await api.approveCommissionReview(conversationId);
    } catch (err) {
      console.error('Failed to approve commission review:', err);
      throw err;
    }
  };

  const handleRejectCommissionReview = async () => {
    if (!conversationId) return;
    try {
      await api.rejectCommissionReview(conversationId);
    } catch (err) {
      console.error('Failed to reject commission review:', err);
      throw err;
    }
  };

  const handleTaskFeedback = async (annotations: string) => {
    if (!conversationId || isArchived) return;
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

  const handleCloseFileViewer = useCallback(() => {
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

  // External draft insertion trigger fired by surfaces that don't hold a ref
  // to the composer (skill viewer, message context menu). Preserve existing
  // user text by appending instead of replacing.
  // Dispatching into `DraftStore` works regardless of whether `<InputArea>`
  // is currently mounted — narrow-desktop fullscreen flows unmount it.
  useEffect(() => {
    const handler = (e: Event) => {
      const text = (e as CustomEvent<{ text: string }>).detail?.text;
      if (!text) return;
      appendDraftCb(text);
      requestComposerFocus();
    };
    window.addEventListener('phoenix:insert-draft', handler);
    return () => window.removeEventListener('phoenix:insert-draft', handler);
  }, [appendDraftCb, requestComposerFocus]);

  useEffect(() => {
    const handler = (e: Event) => {
      const sequenceId = (e as CustomEvent<{ sequenceId: number }>).detail?.sequenceId;
      if (!Number.isSafeInteger(sequenceId) || sequenceId <= 0) return;
      handleOpenMessageViewer(sequenceId);
    };
    window.addEventListener(OPEN_MESSAGE_VIEWER_EVENT, handler);
    return () => window.removeEventListener(OPEN_MESSAGE_VIEWER_EVENT, handler);
  }, [handleOpenMessageViewer]);

  const handleSendNotes = useCallback(
    (formattedNotes: string) => {
      // Dispatching into `DraftStore` works the same whether `<InputArea>`
      // is currently mounted (right-pane / mobile-overlay flow) or unmounted
      // (narrow-desktop fullscreen flow — the viewer closes immediately
      // below). `requestComposerFocus()` is a token bump that InputArea
      // consumes on its next render, including after a remount.
      appendDraftCb(formattedNotes);
      requestComposerFocus();
      fileExplorer.closeFile();
    },
    [fileExplorer, appendDraftCb, requestComposerFocus]
  );

  const handleOpenFileFromPatch = useCallback(
    (filePath: string, modifiedLines: Set<number>, firstModifiedLine: number, focusEndLine?: number) => {
      const rootDir = conversation?.worktree_path ?? conversation?.cwd ?? '/';
      const fullPath = filePath.startsWith('/') ? filePath : `${rootDir}/${filePath}`;
      if (focusEndLine !== undefined && firstModifiedLine > 0) {
        fileExplorer.openFile(fullPath, rootDir, {
          kind: 'range',
          startLine: firstModifiedLine,
          endLine: focusEndLine,
        });
        return;
      }
      fileExplorer.openFile(fullPath, rootDir, { kind: 'patch', patchContext: { modifiedLines, firstModifiedLine } });
    },
    [conversation?.worktree_path, conversation?.cwd, fileExplorer]
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

  // Sub-agent parent breadcrumb: mirrors the seed-parent link so a sub-agent
  // opened as a full page (deep link, or the panel's "open as full page"
  // escape hatch) still has a way back to the conversation that spawned it.
  const parentConvSlugForCallback = conversation?.parent_conversation_slug;
  const handleParentConvClick = useCallback((e: ReactMouseEvent) => {
    if (!parentConvSlugForCallback) return;
    e.preventDefault();
    navigate(`/c/${parentConvSlugForCallback}`);
  }, [parentConvSlugForCallback, navigate]);

  const convStateForChildren = atom.phase;
  const localCreateIntent = readCreateIntent(conversationId);
  const provisioningPrompt = convStateForChildren.type === 'provisioning'
    ? (convStateForChildren.prompt ?? conversation?.creation_prompt ?? localCreateIntent?.prompt ?? null)
    : null;
  const creationFailedPrompt = convStateForChildren.type === 'creation_failed'
    ? (convStateForChildren.prompt ?? conversation?.creation_prompt ?? localCreateIntent?.prompt ?? null)
    : null;
  const creationCancelledPrompt = convStateForChildren.type === 'creation_cancelled'
    ? (convStateForChildren.prompt ?? conversation?.creation_prompt ?? localCreateIntent?.prompt ?? null)
    : null;
  const creationFailedDraft = convStateForChildren.type === 'creation_failed' || convStateForChildren.type === 'creation_cancelled'
    ? (localCreateIntent?.prompt ?? convStateForChildren.prompt ?? conversation?.creation_prompt ?? null)
    : null;
  const handleStartOverFromFailedCreation = useCallback(() => {
    const prompt = creationFailedDraft;
    if (prompt) {
      try {
        localStorage.setItem('phoenix-new-conversation-draft', prompt);
      } catch {
        // ignore — non-fatal
      }
    }
    navigate('/new');
  }, [creationFailedDraft, navigate]);
  const handleDeleteProvisioningConversation = useCallback(async () => {
    if (!conversationId || deletingConversation) return;
    setDeletingConversation(true);
    try {
      await api.deleteConversation(conversationId);
      await cacheDB.deleteConversation(conversationId);
      clearCreateIntent(conversationId);
      navigate('/');
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to delete conversation');
      setDeletingConversation(false);
    }
  }, [conversationId, deletingConversation, navigate]);

  useEffect(() => {
    if (convStateForChildren.type === 'provisioning') return;
    if (convStateForChildren.type !== 'creation_failed' && convStateForChildren.type !== 'creation_cancelled') {
      clearCreateIntent(conversationId);
    }
  }, [convStateForChildren.type, conversationId]);
  const handleSendTextOnly = useCallback((text: string) => handleSend(text, []), [handleSend]);
  const fileRootPath = routePrefix === '/global' || isArchived || !conversation
    ? null
    : (conversation.worktree_path ?? conversation.cwd);
  const handleOpenFiles = useCallback(() => {
    if (fileRootPath) setShowFileBrowser(true);
  }, [fileRootPath]);
  useEffect(() => {
    if (!fileRootPath) setShowFileBrowser(false);
  }, [fileRootPath]);
  const openFileState = fileRootPath ? fileExplorer.openFileState : null;
  const browserViewerOpen = !isArchived && browserOpen;
  const inspectViewerOpen = !isArchived && inspectOpen;
  const canOpenMessageSidepanel = !isArchived && !isTerminalConversationState(convStateForChildren);
  const messageViewerOpen = canOpenMessageSidepanel && messageOpen;
  const commissionReviewViewerOpen = canShowCommissionReviewViewer(canOpenMessageSidepanel, commissionReviewOpen, atom.phase.type);
  const stateBarContinuation = useMemo(
    () => !isArchived && convStateForChildren.type === 'idle'
      ? { phase: 'idle' as const, onTrigger: handleTriggerContinuation }
      : { phase: 'unavailable' as const },
    [isArchived, convStateForChildren.type, handleTriggerContinuation],
  );

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
    if (openFileState) {
      const prs = openFileState;
      return (
        <div id="app">
          <Suspense fallback={null}>
            <FileViewer
              filePath={prs.path}
              rootDir={prs.rootDir}
              onClose={handleCloseFileViewer}
              onSendNotes={handleSendNotes}
              patchContext={prs.patchContext ?? undefined}
              focus={prs.focus}
              inline
            />
          </Suspense>
        </div>
      );
    }
    if (paneDiffOpen && conversationId) {
      return (
        <div id="app">
          <Suspense fallback={null}>
            <ConversationDiffViewer
              conversationId={conversationId}
              target={diffTarget}
              activePrIdentity={activePrIdentity}
              onClose={handleCloseDiff}
              onSendNotes={handleSendNotes}
              inline
            />
          </Suspense>
        </div>
      );
    }
    if (browserViewerOpen && conversationId) {
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
    if (inspectViewerOpen && inspectSlot) {
      return (
        <div id="app">
          <Suspense fallback={null}>
            <ProcessInspectorPanel
              scopeKey={inspectSlot.scopeKey}
              handleId={inspectSlot.handleId}
              onClose={handleCloseInspector}
              inline
            />
          </Suspense>
        </div>
      );
    }
    if (messageViewerOpen && messageSlot) {
      return (
        <div id="app">
          <Suspense fallback={null}>
            <MessageViewer
              sequenceId={messageSlot.sequenceId}
              messages={viewableMessages}
              onClose={handleCloseMessageViewer}
              onSendNotes={handleSendNotes}
              inline
            />
          </Suspense>
        </div>
      );
    }
    if (commissionReviewViewerOpen && commissionReviewSlot) {
      return (
        <div id="app">
          <Suspense fallback={null}>
            <CommissionReviewViewer
              sequenceId={commissionReviewSlot.requestSequenceId}
              messages={viewableMessages}
              onClose={handleCloseMessageViewer}
              inline
            />
          </Suspense>
        </div>
      );
    }
  }

  // Terminal cleanup (Clean up / Abandon) for a Work/Branch
  // conversation stuck in a disposable phase (error or context-exhausted): the
  // backend and specs permit TaskResolved from those states. PR-aware; renders
  // nothing for non-Work/Branch conversations; once continued the actions
  // disable with a tooltip. Deliberately no onSendMessage — a stuck
  // conversation exposes only terminal cleanup, never a message-posting action
  // that would reopen the error. One definition, shared by both stuck branches.
  const stuckCleanupBar = conversationId && !isArchived && (
    <WorkControlBar
      conversationId={conversationId}
      convModeLabel={conversation.conv_mode_label}
      phaseType={convStateForChildren.type}
      continuedInConvId={conversation.continued_in_conv_id}
      showError={showError}
      prStatusHandle={prStatusHandle}
    />
  );
  const showTerminal =
    !!conversationId &&
    !isArchived &&
    convStateForChildren.type !== 'terminal' &&
    convStateForChildren.type !== 'handed_off' &&
    convStateForChildren.type !== 'provisioning' &&
    convStateForChildren.type !== 'creation_failed' &&
    convStateForChildren.type !== 'creation_cancelled' &&
    convStateForChildren.type !== 'context_exhausted';

  // Derived: model context window is a pure function of the current model's
  // spec. Falls back to 200_000 for legacy surfaces when availableModels hasn't
  // loaded yet or the model isn't in the registry.
  const matchingModel = availableModels
    ? availableModels.find((model) => model.id === atom.conversation?.model)
    : undefined;
  const actualModelContextWindow = matchingModel ? matchingModel.context_window : null;
  const modelContextWindow = actualModelContextWindow ?? 200_000;

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

  const parentConvBreadcrumb = conversation.parent_conversation_id ? (
    <div className="conversation-seed-breadcrumb">
      {conversation.parent_conversation_slug ? (
        <a href={`/c/${conversation.parent_conversation_slug}`} onClick={handleParentConvClick}>
          {'\u2190'} sub-agent of: {conversation.parent_conversation_slug}
        </a>
      ) : (
        <span>
          {'\u2190'} sub-agent of: (parent deleted)
        </span>
      )}
    </div>
  ) : null;

  // Split-pane viewer: rendered inside `#app` as a sibling of
  // .conversation-column when wide-desktop and a viewer (file OR diff)
  // is open. CSS in .app-split-pane (index.css) flexes children
  // horizontally.
  const splitPanePrs = openFileState;
  const showSplitPaneViewer =
    isDesktop
    && isWideDesktop
    && (splitPanePrs !== null || paneDiffOpen || browserViewerOpen || inspectViewerOpen || messageViewerOpen || commissionReviewViewerOpen);

  const terminalSplitPane = showTerminal && routePrefix !== '/global' ? (
    <>
      <PaneDivider
        orientation="horizontal"
        title="Drag to resize • Double-click to collapse/expand"
        onPointerDown={(e) => terminalPane.startDrag(e, 'y', true, handleTerminalLiveResize)}
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
          scope={{ kind: 'conversation', conversationId: conversationId! }}
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
  ) : null;

  const stateBarConversation = routePrefix === '/global' && conversation
    ? { ...conversation, cwd: '', worktree_path: null }
    : conversation;

  return (
    <ForkProposalsProvider
      conversationId={conversationId}
      originTerminal={isArchived || isTerminalConversationState(convStateForChildren)}
      onOutcome={handleForkOutcome}
      onError={showError}
    >
    <div
      id="app"
      ref={setAppElement}
      className={showSplitPaneViewer ? 'app-split-pane' : undefined}
    >
      <div className="conversation-column">
      {seedBreadcrumb}
      {parentConvBreadcrumb}
      {viewerSlot.browserSessionActive && !isArchived && !browserOpen && (
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
      <RenderProfiler id="MessageList">
      <ConversationNavStack
        messages={atom.messages}
        pendingMessages={pendingMessages}
        convState={convStateForChildren}
        onRetry={handleRetry}
        onCancelSteering={isArchived ? undefined : handleCancelSteering}
        onOpenFile={isArchived ? undefined : handleOpenFileFromPatch}
        onOpenCommissionReview={canOpenMessageSidepanel ? handleOpenCommissionReview : undefined}
        filePathRootDir={conversation.worktree_path ?? conversation.cwd ?? '/'}
        workScopeKey={isArchived ? undefined : conversation.work_scope_key}
        enableMessageSidepanel={canOpenMessageSidepanel}
        conversationId={conversationId}
        slug={slug}
        systemPrompt={atom.systemPrompt ?? undefined}
        hasOlderMessages={historyExpansion.coverage === 'tail'}
        onLoadOlderMessages={loadOlderMessages}
        onUpdateOlderMessagesRestore={updateOlderMessagesRestore}
        loadingOlderMessages={historyExpansion.activeRequest !== null}
        olderHistoryError={historyExpansion.failure?.kind === 'request_failed'
          ? historyExpansion.failure.message
          : historyExpansion.failure?.kind === 'target_not_found'
            ? 'The requested message is not in this conversation.'
            : historyExpansion.failure?.kind === 'anchor_not_found'
              ? 'Could not preserve the previous reading position.'
              : null}
        transcriptPositioning={transcriptPositioningInputFromHistoryExpansion(historyExpansion)}
        onHistoryScrollCommandHandled={handleHistoryScrollCommand}
      />
      </RenderProfiler>
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
      {convStateForChildren.type === 'provisioning' && (
        <div className="terminal-banner">
          <span>
            Creating conversation… shell ready{provisioningPrompt ? ' • prompt queued' : ''}
          </span>
          {provisioningPrompt && (
            <button
              type="button"
              className="context-exhausted-copy"
              onClick={async () => {
                const ok = await copyToClipboard(provisioningPrompt);
                showInfo(ok ? 'Queued prompt copied to clipboard' : 'Copy failed -- select and copy manually');
              }}
            >
              Copy prompt
            </button>
          )}
          <button
            type="button"
            className="context-exhausted-copy"
            onClick={() => void handleCancel()}
          >
            Cancel
          </button>
          <button
            type="button"
            className="context-exhausted-copy"
            disabled={deletingConversation}
            onClick={() => void handleDeleteProvisioningConversation()}
          >
            {deletingConversation ? 'Deleting…' : 'Delete'}
          </button>
        </div>
      )}
      {convStateForChildren.type === 'creation_failed' && (
        <div className="context-exhausted-banner context-exhausted-banner--expanded">
          <div className="context-exhausted-summary">
            <div className="error-body-title">Conversation creation failed</div>
            <div className="error-body-details">
              {convStateForChildren.message ?? 'Phoenix created the shell but could not finish setup.'}
            </div>
            <div className="context-exhausted-actions">
              <button type="button" className="context-exhausted-continue" onClick={handleStartOverFromFailedCreation}>
                Start over
              </button>
              <button
                type="button"
                className="context-exhausted-copy"
                disabled={deletingConversation}
                onClick={() => void handleDeleteProvisioningConversation()}
              >
                {deletingConversation ? 'Deleting…' : 'Delete'}
              </button>
              {creationFailedPrompt && (
                <button
                  type="button"
                  className="context-exhausted-copy"
                  onClick={async () => {
                    const ok = await copyToClipboard(creationFailedPrompt);
                    showInfo(ok ? 'Prompt copied to clipboard' : 'Copy failed -- select and copy manually');
                  }}
                >
                  Copy prompt
                </button>
              )}
            </div>
            {creationFailedPrompt && (
              <pre className="context-exhausted-content">{creationFailedPrompt}</pre>
            )}
          </div>
        </div>
      )}
      {convStateForChildren.type === 'creation_cancelled' && (
        <div className="context-exhausted-banner context-exhausted-banner--expanded">
          <div className="context-exhausted-summary">
            <div className="error-body-title">Conversation creation cancelled</div>
            <div className="context-exhausted-actions">
              <button type="button" className="context-exhausted-continue" onClick={handleStartOverFromFailedCreation}>
                Start over
              </button>
              <button
                type="button"
                className="context-exhausted-copy"
                disabled={deletingConversation}
                onClick={() => void handleDeleteProvisioningConversation()}
              >
                {deletingConversation ? 'Deleting…' : 'Delete'}
              </button>
            </div>
            {creationCancelledPrompt && (
              <pre className="context-exhausted-content">{creationCancelledPrompt}</pre>
            )}
          </div>
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
              {!isArchived && (conversation.continued_in_conv_id ? (
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
                        navigate(`${routePrefix}/${res.conversation_id}`);
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
                        navigate(`${routePrefix}/${res.conversation_id}`);
                      }
                    } catch (err) {
                      showInfo(err instanceof Error ? err.message : 'Failed to start new conversation');
                    }
                  }}
                >
                  Continue in new conversation
                </button>
              ))}
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
            </div>
            {stuckCleanupBar}
            {contextExhaustedExpanded && (
              <pre className="context-exhausted-content">
                {convStateForChildren.summary}
              </pre>
            )}
          </div>
        </div>
      )}
      {convStateForChildren.type === 'handed_off' && (
        <div className="terminal-banner">
          <span>Task approved. Work started in a fresh conversation.</span>
          <button
            type="button"
            className="context-exhausted-continue"
            onClick={async () => {
              const successorId =
                convStateForChildren.type === 'handed_off'
                  ? convStateForChildren.successor_conv_id
                  : null;
              if (!successorId) return;
              try {
                const slug = await api.getConversationSlug(successorId);
                if (slug) navigate(`/c/${slug}`);
                else showInfo('Work conversation no longer exists');
              } catch (err) {
                showInfo(err instanceof Error ? err.message : 'Failed to open work conversation');
              }
            }}
          >
            {'→'} Open work conversation
          </button>
        </div>
      )}
      {convStateForChildren.type === 'awaiting_recovery' && isArchived ? (
        <RecoveryBanner message={convStateForChildren.message} recoveryKind={convStateForChildren.recovery_kind} />
      ) : convStateForChildren.type === 'awaiting_recovery' ? (
        <>
        {credentialStatus && (
          <Suspense fallback={null}>
            <CredentialHelperPanel
              active={true}
              onDismiss={() => void refreshModels().catch(() => {})}
            />
          </Suspense>
        )}
        <RenderProfiler id="InputArea">
        <ConnectedInputArea
          ref={inputRef}
          slug={slug!}
          cwd={routePrefix === '/global' ? undefined : conversation.cwd}
          scopeKey={conversationId}
          convState={convStateForChildren}
          images={images}
          setImages={setImages}
          files={files}
          setFiles={setFiles}
          isOffline={isOffline}
          failedMessages={failedMessages}
          convModeLabel={conversation.conv_mode_label}
          focusToken={focusToken}
          onSend={handleSend}
          onCancel={handleCancel}
          onRetry={handleRetry}
          onDismissError={dismiss}
        />
        </RenderProfiler>
        </>
      ) : convStateForChildren.type === 'error' ? (
        <>
        {stuckCleanupBar}
        <ErrorBanner
          message={convStateForChildren.message}
          error={convStateForChildren.error}
          onRetry={isArchived ? undefined : () => handleSend('continue', [])}
          onDismiss={isArchived ? undefined : () => {
            // No optimistic idle: `dismissError` resolves on enqueue, not on
            // persist, so faking idle could diverge if the executor races/
            // rejects. The server-broadcast state_change SSE drives idle.
            void api.dismissError(conversation.id).catch((e) => {
              showError(e instanceof Error ? e.message : 'Failed to dismiss error');
            });
          }}
        />
        {/* A resumable error accepts a fresh user message (backend transitions
            Error -> LlmRequesting). Offer the real composer alongside the
            banner's quick "retry/continue" so the user is not forced into the
            canned "continue" turn. Gated on can_user_resume to match the
            banner: a non-resumable error stays a dead end. */}
        {(convStateForChildren.error?.can_user_resume ?? false) && !isArchived && (
          <RenderProfiler id="InputArea">
          <ConnectedInputArea
            ref={inputRef}
            slug={slug!}
            cwd={routePrefix === '/global' ? undefined : conversation.cwd}
            scopeKey={conversationId}
            convState={convStateForChildren}
            images={images}
            setImages={setImages}
            files={files}
            setFiles={setFiles}
            isOffline={isOffline}
            failedMessages={failedMessages}
            convModeLabel={conversation.conv_mode_label}
            focusToken={focusToken}
            onSend={handleSend}
            onCancel={handleCancel}
            onRetry={handleRetry}
            onDismissError={dismiss}
          />
          </RenderProfiler>
        )}
        </>
      ) : convStateForChildren.type === 'awaiting_user_response' ? (
        <QuestionPanel
          questions={convStateForChildren.questions}
          conversationId={conversation.id}
          showToast={showInfo}
          readOnly={isArchived}
          onAnswered={() => dispatch({ type: 'local_phase_change', phase: { type: 'llm_requesting', attempt: 1 }, expectedConversationId: conversation.id })}
          onDismissed={() => dispatch({ type: 'local_phase_change', phase: { type: 'idle' }, expectedConversationId: conversation.id })}
        />
      ) : !isArchived && convStateForChildren.type !== 'provisioning' && convStateForChildren.type !== 'creation_failed' && convStateForChildren.type !== 'creation_cancelled' && convStateForChildren.type !== 'context_exhausted' && convStateForChildren.type !== 'awaiting_task_approval' && convStateForChildren.type !== 'handed_off' && convStateForChildren.type !== 'terminal' ? (
        <>
        {conversationId && (
          <WorkControlBar
            conversationId={conversationId}
            convModeLabel={conversation.conv_mode_label}
            phaseType={convStateForChildren.type}
            continuedInConvId={conversation.continued_in_conv_id}
            onSendMessage={handleSendTextOnly}
            showError={showError}
            prStatusHandle={prStatusHandle}
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
        {routePrefix !== '/global' && (
          <ExploreOnboardingBanner
            convModeLabel={conversation.conv_mode_label}
            messageCount={conversation.message_count}
          />
        )}
        <RenderProfiler id="InputArea">
        <ConnectedInputArea
          ref={inputRef}
          slug={slug!}
          cwd={routePrefix === '/global' ? undefined : conversation.cwd}
          scopeKey={conversationId}
          convState={convStateForChildren}
          images={images}
          setImages={setImages}
          files={files}
          setFiles={setFiles}
          isOffline={isOffline}
          failedMessages={failedMessages}
          convModeLabel={conversation.conv_mode_label}
          focusToken={focusToken}
          onSend={handleSend}
          onCancel={handleCancel}
          onRetry={handleRetry}
          onDismissError={dismiss}
        />
        </RenderProfiler>
        </>
      ) : null}
      {!isDesktop && terminalSplitPane}
      <RenderProfiler id="StateBar">
      <ConnectedStateBar
        slug={slug!}
        conversation={stateBarConversation as Conversation}
        convState={convStateForChildren}
        connectionState={connectionInfo.state}
        connectionAttempt={connectionInfo.attempt}
        nextRetryIn={connectionInfo.nextRetryIn}
        contextWindowUsed={atom.contextWindow.used}
        modelContextWindow={modelContextWindow}
        availableModels={availableModels}
        onRetryNow={connectionInfo.retryNow}
        continuation={stateBarContinuation}
        onUpgradeModel={handleUpgradeModel}
        toolExecutingStartedAt={atom.toolExecutingStartedAt}
        phaseStateUpdatedAt={atom.phaseStateUpdatedAt}
        firstByteRequestId={atom.firstByteRequestId}
        turnRetryContext={atom.turnRetryContext}
        onOpenFiles={isDesktop || !fileRootPath ? undefined : handleOpenFiles}
        prStatusHandle={prStatusHandle}
      />
      </RenderProfiler>
      </div>

      {/* Terminal split-pane (REQ-TERM-001) — collapsed = 32px header strip.
          Lazy-loaded so xterm (~200KB) stays out of the main bundle. */}
      {isDesktop && terminalSplitPane}

      {/* Task approval overlay — browser back navigates away; SSE restores state on return. */}
      {showTaskApproval && !isArchived && atom.phase.type === 'awaiting_task_approval' && (
        <Suspense fallback={null}>
          <TaskApprovalReader
            title={atom.phase.title}
            priority={atom.phase.priority}
            plan={atom.phase.plan}
            contextWindowUsed={approvalContextWindowUsed ?? undefined}
            modelContextWindow={actualModelContextWindow ?? undefined}
            approvalError={taskApprovalError}
            onApprove={handleApproveTask}
            onReject={handleRejectTask}
            onSendFeedback={handleTaskFeedback}
          />
        </Suspense>
      )}
      {atom.phase.type === 'awaiting_commission_review_approval' && (
        <Suspense fallback={null}>
          <CommissionReviewApproval
            brief={atom.phase.brief}
            focus={atom.phase.focus}
            scope={atom.phase.scope}
            onApprove={handleApproveCommissionReview}
            onReject={handleRejectCommissionReview}
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
      {fileRootPath && (
        <FileBrowserOverlay
          isOpen={showFileBrowser}
          rootPath={fileRootPath}
          conversationId={conversation.id}
          onClose={() => setShowFileBrowser(false)}
          onFileSelect={handleFileSelect}
        />
      )}

      {/* Mobile prose reader overlay — reads URL-driven state from
          FileExplorerProvider so cold reload (e.g. iOS PWA return) restores
          the exact file the user was viewing. */}
      {!isDesktop && openFileState && (
        <Suspense fallback={null}>
          <FileViewer
            filePath={openFileState.path}
            rootDir={openFileState.rootDir}
            onClose={handleCloseFileViewer}
            onSendNotes={handleSendNotes}
            patchContext={openFileState.patchContext ?? undefined}
            focus={openFileState.focus}
          />
        </Suspense>
      )}
      {/* Fullscreen diff takeover: dismissible review surface above app chrome.
          Pane presentation remains available for intentional split-pane callers. */}
      {fullscreenDiffOpen && conversationId && (
        <Suspense fallback={null}>
          <ConversationDiffViewer
            conversationId={conversationId}
            target={diffTarget}
            activePrIdentity={activePrIdentity}
            onClose={handleCloseDiff}
            onSendNotes={handleSendNotes}
            takeover
          />
        </Suspense>
      )}
      {paneDiffOpen && !showSplitPaneViewer && conversationId && (
        <Suspense fallback={null}>
          <ConversationDiffViewer
            conversationId={conversationId}
            target={diffTarget}
            activePrIdentity={activePrIdentity}
            onClose={handleCloseDiff}
            onSendNotes={handleSendNotes}
          />
        </Suspense>
      )}
      {/* Browser view overlay: same fallback role as the diff overlay above
          — mobile, narrow desktop, or any case where the split pane is
          unavailable. REQ-BT-018. */}
      {browserViewerOpen && !showSplitPaneViewer && conversationId && (
        <Suspense fallback={null}>
          <div className="browser-view-overlay">
            <BrowserViewPanel
              conversationId={conversationId}
              onClose={handleCloseBrowserView}
            />
          </div>
        </Suspense>
      )}
      {/* Process inspector overlay: mobile, narrow desktop, or any case where
          the split pane is unavailable (REQ-PINSP-007). */}
      {inspectViewerOpen && inspectSlot && !showSplitPaneViewer && (
        <Suspense fallback={null}>
          <ProcessInspectorPanel
            scopeKey={inspectSlot.scopeKey}
            handleId={inspectSlot.handleId}
            onClose={handleCloseInspector}
          />
        </Suspense>
      )}
      {messageViewerOpen && messageSlot && !showSplitPaneViewer && (
        <Suspense fallback={null}>
          <MessageViewer
            sequenceId={messageSlot.sequenceId}
            messages={viewableMessages}
            onClose={handleCloseMessageViewer}
            onSendNotes={handleSendNotes}
          />
        </Suspense>
      )}
      {commissionReviewViewerOpen && commissionReviewSlot && !showSplitPaneViewer && (
        <Suspense fallback={null}>
          <CommissionReviewViewer
            sequenceId={commissionReviewSlot.requestSequenceId}
            messages={viewableMessages}
            onClose={handleCloseMessageViewer}
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
            onPointerDown={(e) => viewerPane.startDrag(e, 'x', true, handleViewerLiveResize)}
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
              {paneDiffOpen && conversationId ? (
                <ConversationDiffViewer
                  conversationId={conversationId}
                  target={diffTarget}
                  activePrIdentity={activePrIdentity}
                  onClose={handleCloseDiff}
                  onSendNotes={handleSendNotes}
                  inline
                />
              ) : splitPanePrs ? (
                <FileViewer
                  filePath={splitPanePrs.path}
                  rootDir={splitPanePrs.rootDir}
                  onClose={handleCloseFileViewer}
                  onSendNotes={handleSendNotes}
                  patchContext={splitPanePrs.patchContext ?? undefined}
                  focus={splitPanePrs.focus}
                  inline
                />
              ) : browserViewerOpen && conversationId ? (
                <BrowserViewPanel
                  conversationId={conversationId}
                  onClose={handleCloseBrowserView}
                  inline
                />
              ) : inspectViewerOpen && inspectSlot ? (
                <ProcessInspectorPanel
                  scopeKey={inspectSlot.scopeKey}
                  handleId={inspectSlot.handleId}
                  onClose={handleCloseInspector}
                  inline
                />
              ) : messageViewerOpen && messageSlot ? (
                <MessageViewer
                  sequenceId={messageSlot.sequenceId}
                  messages={viewableMessages}
                  onClose={handleCloseMessageViewer}
                  onSendNotes={handleSendNotes}
                  inline
                />
              ) : commissionReviewViewerOpen && commissionReviewSlot ? (
                <CommissionReviewViewer
                  sequenceId={commissionReviewSlot.requestSequenceId}
                  messages={viewableMessages}
                  onClose={handleCloseMessageViewer}
                  inline
                />
              ) : null}
            </Suspense>
          </div>
        </>
      )}
    </div>
    {/* Fork proposal review modal — full-screen, driven by the ForkProposals
        store's openProposalId (REQ-PROJ-034 / 037). */}
    <ForkReviewOverlay />
    </ForkProposalsProvider>
  );
}

/** Mounts the fork-proposal review modal when the store has an open proposal.
 *  Lives inside <ForkProposalsProvider> so it can read the open id + actions. */
function ForkReviewOverlay() {
  const fork = useForkProposals();
  if (!fork || !fork.openProposalId) return null;
  const proposal = fork.getProposal(fork.openProposalId);
  // Only `pending` proposals are reviewable; a resolved one (e.g. another tab
  // acted, then the list refetched) withdraws the modal.
  if (!proposal || proposal.status !== 'pending') return null;
  const proposalId = proposal.id;
  return (
    <Suspense fallback={null}>
      <ForkProposalReview
        proposal={proposal}
        onApprove={() => fork.approve(proposalId)}
        onDismiss={() => fork.dismiss(proposalId)}
        onRequestChanges={(note) => fork.requestChanges(proposalId, note)}
        onClose={fork.closeReview}
      />
    </Suspense>
  );
}
