import { memo, useState, useRef, useCallback, useMemo, useEffect, forwardRef, useImperativeHandle } from 'react';
import { Virtuoso, type VirtuosoHandle, type VirtuosoProps, type ListRange } from 'react-virtuoso';
import type { Message, ConversationState } from '../api';
import type { QueuedMessage } from '../hooks';
import {
  UserMessage,
  QueuedUserMessage,
  AgentMessage,
  SubAgentStatus,
  SkillCommandText,
  formatMessageTime,
} from './MessageComponents';
import { StreamingMessage } from './StreamingMessage';
import { RenderProfiler } from '../dev/renderProfiler';
import { MessageContextMenu } from './MessageContextMenu';
import { FilePathContextMenu } from './FilePathContextMenu';
import { useStreamingRequestId } from '../conversation/useConversationAtom';
import {
  buildRenderUnits,
  type HistoricalUnit,
  type TailUnit,
  type RenderUnit,
} from '../conversation/renderUnits';
import {
  buildConversationChapters,
  type Chapter,
} from '../conversation/conversationChapters';
import { ensureTargetTopVisible } from './jumpScroll';

const ChevronRight = () => (
  <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <polyline points="9 18 15 12 9 6" />
  </svg>
);
const ChevronDown = () => (
  <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <polyline points="6 9 12 15 18 9" />
  </svg>
);
const MessageSquareIcon = () => (
  <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
  </svg>
);

interface MessageListProps {
  messages: Message[];
  pendingMessages: QueuedMessage[];
  convState: ConversationState;
  onRetry: (localId: string) => void;
  onCancelSteering?: ((localId: string) => void) | undefined;
  onOpenFile: ((filePath: string, modifiedLines: Set<number>, firstModifiedLine: number) => void) | undefined;
  systemPrompt?: string | undefined;
  conversationId?: string | undefined;
  slug?: string | undefined;
  filePathRootDir?: string | undefined;
  workScopeKey?: string | undefined;
  /** Scroll-spy: the inclusive range of `historicalUnits`/virtuoso item
   *  indices currently rendered. Fired (debounced by virtuoso) as the user
   *  scrolls. The conversation nav uses it to highlight the active chapter. */
  onVisibleRangeChange?: ((range: ListRange) => void) | undefined;
  /** Conversation chapters derived from the SAME `historicalUnits` array this
   *  list feeds to virtuoso. MessageList owns the build, so the chapter
   *  `unitIndex` values are guaranteed to be in virtuoso's coordinate space —
   *  no second `buildRenderUnits` pass to drift against. */
  onChaptersChange?: ((chapters: Chapter[]) => void) | undefined;
}

/** Imperative surface exposed to the conversation nav strip. MessageList owns
 *  `virtuosoRef`; the nav can't reach it directly because off-screen rows are
 *  unmounted (react-virtuoso), so a querySelector jump would miss them. */
export interface MessageListHandle {
  /** Scroll the render unit at `unitIndex` (a `historicalUnits` index, which
   *  equals its virtuoso item index) into view and pulse it once mounted. */
  scrollToUnitIndex: (unitIndex: number) => void;
}


const PIN_TO_BOTTOM_THRESHOLD = 100;

// How long after the last upward user scroll the auto-follow re-snap stays
// suppressed. Rolling: touch momentum keeps refreshing it on every upward
// scroll event, so the window only has to outlive the gap between momentum
// scroll events, not the whole momentum animation.
const USER_SCROLL_SUPPRESS_MS = 400;

// Settle watch: how long after a conversation's first measurement the list
// keeps verifying it is pinned to the bottom, and how often. Mount-time
// stranding can be silent — virtuoso's placement churn does not always end
// with a height delta or scroll event to hook — so event-driven rescue
// alone can leave the viewport stranded until an unrelated late
// measurement. The watch is stopped early by any user engagement.
const SETTLE_WATCH_MS = 3000;
const SETTLE_WATCH_INTERVAL_MS = 150;

function formatAttachmentBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function SkillFileChips({ files }: { files: { original_name: string; size_bytes: number; stored_path?: string }[] }) {
  if (files.length === 0) return null;
  return (
    <div className="message-files">
      {files.map((file, idx) => (
        <span key={`${file.stored_path ?? file.original_name}-${idx}`} className="message-file-chip" title={file.stored_path}>
          📎 {file.original_name} <span className="message-file-size">{formatAttachmentBytes(file.size_bytes)}</span>
        </span>
      ))}
    </div>
  );
}

type OnOpenFile = ((filePath: string, modifiedLines: Set<number>, firstModifiedLine: number) => void) | undefined;

function activeToolUseIdFromState(convState: ConversationState): string | undefined {
  if (convState.type !== 'tool_executing' && convState.type !== 'cancelling_tool') return undefined;
  const id = convState.current_tool?.id;
  return typeof id === 'string' && id.length > 0 ? id : undefined;
}

function renderHistoricalUnit(
  unit: HistoricalUnit,
  onOpenFile: OnOpenFile,
  filePathRootDir: string | undefined,
  onRetry: (localId: string) => void,
  onCancelSteering: ((localId: string) => void) | undefined,
  workScopeKey: string | undefined,
  activeToolUseId: string | undefined,
): JSX.Element | null {
  switch (unit.kind) {
    case 'user':
      return <UserMessage message={unit.message} />;
    case 'pending_user':
      return (
        <QueuedUserMessage
          message={unit.message}
          onRetry={onRetry}
          onCancelSteering={onCancelSteering}
        />
      );
    case 'skill': {
      const c = unit.message.content as { name?: string; trigger?: string; args?: string; source?: string; snippet?: string; files?: { original_name: string; size_bytes: number; stored_path?: string }[] };
      const trigger = c.trigger?.trim() || [c.name ? `/${c.name}` : '/skill', c.args?.trim()].filter(Boolean).join(' ');
      return (
        <div className="message user" data-sequence-id={unit.message.sequence_id}>
          <div className="message-header">
            <span className="message-sender">You</span>
            {unit.message.created_at && (
              <span className="message-time" title={new Date(unit.message.created_at).toLocaleString()}>
                {formatMessageTime(unit.message.created_at)}
              </span>
            )}
          </div>
          <div className="message-content">
            <SkillCommandText text={trigger} source={c.source} snippet={c.snippet} />
            <SkillFileChips files={c.files ?? []} />
          </div>
        </div>
      );
    }
    case 'agent_turn':
      return (
        <AgentMessage
          message={unit.agent}
          toolResults={unit.toolResultsByUseId}
          onOpenFile={onOpenFile}
          filePathRootDir={filePathRootDir}
          workScopeKey={workScopeKey}
          activeToolUseId={activeToolUseId}
          isFirstInTurn={unit.isFirstInTurn}
        />
      );
    case 'system': {
      const displayData = unit.message.display_data as
        | { hidden?: boolean }
        | null;
      if (displayData?.hidden) return null;
      const text = (unit.message.content as { text?: string })?.text;
      if (!text) return null;
      return (
        <div className="system-message">
          <span className="system-message-text">{text}</span>
        </div>
      );
    }
  }
}

function renderTailUnit(
  unit: TailUnit,
  slug: string | undefined,
): JSX.Element | null {
  switch (unit.kind) {
    case 'sub_agent_status':
      return <SubAgentStatus stateData={unit.state} />;
    case 'streaming_agent':
      if (!slug) return null;
      return (
        <RenderProfiler id="StreamingMessage">
          <StreamingMessage slug={slug} isFirstInTurn={unit.isFirstInTurn} />
        </RenderProfiler>
      );
  }
}

function renderUnit(
  unit: RenderUnit,
  slug: string | undefined,
  onOpenFile: OnOpenFile,
  filePathRootDir: string | undefined,
  onRetry: (localId: string) => void,
  onCancelSteering: ((localId: string) => void) | undefined,
  workScopeKey: string | undefined,
  activeToolUseId: string | undefined,
): JSX.Element | null {
  if (
    unit.kind === 'sub_agent_status' ||
    unit.kind === 'streaming_agent'
  ) {
    return renderTailUnit(unit, slug);
  }
  return renderHistoricalUnit(unit, onOpenFile, filePathRootDir, onRetry, onCancelSteering, workScopeKey, activeToolUseId);
}

interface SystemPromptHeaderProps {
  systemPrompt: string;
  expanded: boolean;
  onToggle: () => void;
}

const SystemPromptHeader = memo(function SystemPromptHeader({
  systemPrompt,
  expanded,
  onToggle,
}: SystemPromptHeaderProps) {
  return (
    <div className="virtuoso-row">
      <div className={`system-prompt-block${expanded ? ' expanded' : ''}`}>
        <div className="system-prompt-header" onClick={onToggle}>
          <span className="system-prompt-label">System prompt</span>
          <span className="system-prompt-toggle">
            {expanded ? <ChevronDown /> : <ChevronRight />}
            {expanded ? ' hide' : ' show'}
          </span>
        </div>
        {expanded && <pre className="system-prompt-content">{systemPrompt}</pre>}
      </div>
    </div>
  );
});

// Per-conversation data the Virtuoso slot components need. Threaded through
// virtuoso's `context` prop so the slot *component types* can stay stable —
// see `VIRTUOSO_COMPONENTS`.
interface MessageListContext {
  systemPrompt: string | undefined;
  systemPromptExpanded: boolean;
  toggleSystemPrompt: () => void;
}

// Virtuoso slot component types are defined once at module scope so their
// identity never changes across renders. A slot whose component *type* is
// recreated per render (e.g. a closure built inside a render-time useMemo that
// depends on system-prompt expansion) forces virtuoso to unmount/remount that
// slot and recompute total list height — a visible scroll hitch. The
// per-conversation data instead arrives via virtuoso's `context` prop, which
// changes the slot's props without changing its type.
function VirtuosoHeaderSlot({ context }: { context?: MessageListContext }) {
  if (!context?.systemPrompt) return null;
  return (
    <SystemPromptHeader
      systemPrompt={context.systemPrompt}
      expanded={context.systemPromptExpanded}
      onToggle={context.toggleSystemPrompt}
    />
  );
}

// Empty-state UI lives in virtuoso's `EmptyPlaceholder` slot rather than a
// parallel branch, so that a systemPrompt (rendered as virtuoso's Header) stays
// visible alongside the "Start a conversation" affordance for a freshly-opened
// conversation with a system prompt and no messages yet.
function VirtuosoEmptySlot() {
  return (
    <div className="empty-state">
      <div className="empty-state-icon"><MessageSquareIcon /></div>
      <p>Start a conversation</p>
    </div>
  );
}

const VIRTUOSO_COMPONENTS: NonNullable<
  VirtuosoProps<RenderUnit, MessageListContext>['components']
> = {
  Header: VirtuosoHeaderSlot,
  EmptyPlaceholder: VirtuosoEmptySlot,
};

function MessageListImpl({
  messages,
  pendingMessages,
  convState,
  onRetry,
  onCancelSteering,
  onOpenFile,
  systemPrompt,
  conversationId,
  slug,
  filePathRootDir,
  workScopeKey,
  onVisibleRangeChange,
  onChaptersChange,
}: MessageListProps, ref: React.ForwardedRef<MessageListHandle>) {
  const [systemPromptExpanded, setSystemPromptExpanded] = useState(false);
  const [isAtBottom, setIsAtBottom] = useState(true);
  // Tail-content unread tracking: bumped when the visible-unit count grows
  // while the user is NOT pinned at bottom. Cleared when isAtBottom flips
  // true or the jump-to-newest button is clicked. This keeps the
  // "↓ New messages" affordance honest: it only shows after new tail
  // content arrived while the user was scrolled up, not on every
  // scroll-up of a static conversation.
  const [hasUnreadTailContent, setHasUnreadTailContent] = useState(false);
  const virtuosoRef = useRef<VirtuosoHandle>(null);
  const scrollerRef = useRef<HTMLElement | null>(null);
  // Most recent `totalListHeightChanged` value virtuoso has reported. Used
  // only to classify a height delta as growth vs shrink (model units
  // compared against model units) — see handleTotalListHeightChanged.
  const prevTotalHeightRef = useRef(0);
  // Previous DOM scrollHeight of the scroller. The pin-distance check must
  // compare scrollTop (DOM units) against the pre-growth bottom in the SAME
  // units: virtuoso's totalListHeight is an estimate-corrected model value
  // that can disagree with the DOM scrollHeight by more than the pin
  // threshold on long conversations (hundreds of not-yet-measured rows'
  // estimate error), which misclassifies a genuinely pinned user as
  // scrolled-up and silently kills auto-follow. Synced at every height
  // callback, so at callback time it still holds the pre-growth DOM height.
  const prevScrollHeightRef = useRef(0);
  // Previous viewport (scroller) clientHeight. Used to detect viewport
  // shrinks (resize, panel expansion, composer growth) so a pinned user
  // stays pinned — see the viewport-shrink handling in
  // handleTotalListHeightChanged.
  const prevClientHeightRef = useRef(0);
  // Tracks whether this conversation instance has seen non-empty content.
  // Used to force a bottom snap on the first non-empty height measurement
  // (when Virtuoso mounted with empty data and messages arrive later).
  // Not reset by the passive conversation-switch useEffect — only by the
  // callback's synchronous conversation-switch handler, which seeds it
  // from `allUnitsLengthRef` so a reopened conversation with messages
  // doesn't get a forced snap.
  const hasSeenContentRef = useRef(false);

  // The streaming buffer's `requestId` IS the eventual agent message_id
  // (server uses the same uuid for both — see `AssistantMessage::new` in
  // crates/phoenix-ide/src/state_machine/state.rs). Keying the streaming
  // unit by this value means the finalized `agent_turn` HistoricalUnit
  // arrives under the same render-unit key, and the transition is an
  // in-place keyed update — virtuoso doesn't observe a key swap, the
  // viewport doesn't drift. Symmetric to pending_user → user.
  const streamingRequestId = useStreamingRequestId(slug);
  const streamingHandle = useMemo(
    () => (streamingRequestId !== null ? { key: streamingRequestId } : null),
    [streamingRequestId],
  );

  const { historicalUnits, tailUnits } = useMemo(
    () => buildRenderUnits({
      messages,
      pendingMessages,
      convState,
      streamingHandle,
    }),
    [messages, pendingMessages, convState, streamingHandle],
  );

  const allUnits = useMemo<RenderUnit[]>(
    () => [...historicalUnits, ...tailUnits],
    [historicalUnits, tailUnits],
  );

  // Chapters are derived here, not in a parent, so they share the exact
  // `historicalUnits` array virtuoso renders — a chapter's `unitIndex` is
  // therefore a valid `scrollToIndex` target with no second build to drift
  // against. Reported up via callback for the nav strip above the list.
  const chapters = useMemo(
    () => buildConversationChapters(historicalUnits),
    [historicalUnits],
  );
  const onChaptersChangeRef = useRef(onChaptersChange);
  onChaptersChangeRef.current = onChaptersChange;
  useEffect(() => {
    onChaptersChangeRef.current?.(chapters);
  }, [chapters]);

  const isEmpty = allUnits.length === 0;
  const activeToolUseId = activeToolUseIdFromState(convState);

  const handleAtBottomStateChange = useCallback((atBottom: boolean) => {
    setIsAtBottom(atBottom);
    // Returning to bottom clears the unread badge regardless of whether
    // the user got there via scroll, click, or virtuoso's followOutput.
    if (atBottom) setHasUnreadTailContent(false);
  }, []);

  // User-gesture tracking for handleTotalListHeightChanged: an active touch
  // drag, or any upward scroll within USER_SCROLL_SUPPRESS_MS (wheel notch,
  // scrollbar drag, touch momentum after finger lift), marks the viewport as
  // user-owned so the auto-follow re-snap never fights the gesture. Downward
  // movement never suppresses — our own programmatic snaps scroll down, and a
  // user scrolling down is heading to the bottom anyway.
  const touchActiveRef = useRef(false);
  const lastUpwardScrollAtRef = useRef(0);
  const lastScrollTopRef = useRef(0);
  const detachGestureListenersRef = useRef<(() => void) | null>(null);
  // False until the user first interacts with this conversation's scroller
  // (touch, wheel, pointer) or triggers a nav jump. While false, the list is
  // still settling from mount and every height delta re-snaps to the bottom
  // regardless of measured distance: virtuoso's initial `LAST` placement can
  // be stranded far from the bottom when a large estimate correction lands
  // right after mount, and a distance-based pin check would classify that
  // stranding as "user scrolled up" and never recover. The mount contract is
  // "open pinned to the newest message"; only a user action releases it.
  const hasUserEngagedRef = useRef(false);
  // Pending settle-snap frame (see scheduleSettleSnap). One in flight at a
  // time; coalesces the burst of height deltas virtuoso emits while
  // measuring a freshly-mounted conversation.
  const settleSnapRafRef = useRef(0);

  // Pre-engagement settle snap. Deliberately NOT virtuoso's
  // `scrollToIndex('LAST')`: that navigates to the *model's* offset for the
  // last item via an internal seek loop that measurement churn can abort
  // mid-flight, leaving the viewport stranded once height deltas stop. A
  // direct DOM assignment cannot be aborted and needs no model: each snap
  // lands at the current DOM bottom, the tail rows mount and measure, any
  // correction fires another height delta, and the loop converges exactly
  // when the list is measured and the viewport is at the bottom. Deferred
  // one frame so virtuoso's compensatory scrollTop adjustment for the
  // triggering delta has already been applied (writing before it would be
  // immediately shifted off-bottom by the compensation).
  const scheduleSettleSnap = useCallback(() => {
    if (settleSnapRafRef.current !== 0) return;
    settleSnapRafRef.current = requestAnimationFrame(() => {
      settleSnapRafRef.current = 0;
      if (hasUserEngagedRef.current) return;
      const s = scrollerRef.current;
      if (!s) return;
      s.scrollTop = s.scrollHeight;
    });
  }, []);

  // Bounded settle watch (see SETTLE_WATCH_MS). Restarting extends the
  // deadline; the interval instance is shared.
  const settleWatchTimerRef = useRef(0);
  const settleWatchDeadlineRef = useRef(0);

  const stopSettleWatch = useCallback(() => {
    if (settleWatchTimerRef.current !== 0) {
      clearInterval(settleWatchTimerRef.current);
      settleWatchTimerRef.current = 0;
    }
  }, []);

  const startSettleWatch = useCallback(() => {
    settleWatchDeadlineRef.current = Date.now() + SETTLE_WATCH_MS;
    scheduleSettleSnap();
    if (settleWatchTimerRef.current !== 0) return;
    settleWatchTimerRef.current = window.setInterval(() => {
      if (
        hasUserEngagedRef.current ||
        Date.now() > settleWatchDeadlineRef.current
      ) {
        stopSettleWatch();
        return;
      }
      const s = scrollerRef.current;
      if (!s) return;
      if (s.scrollHeight - s.scrollTop - s.clientHeight > 1) {
        s.scrollTop = s.scrollHeight;
      }
    }, SETTLE_WATCH_INTERVAL_MS);
  }, [scheduleSettleSnap, stopSettleWatch]);

  useEffect(() => {
    return () => {
      if (settleSnapRafRef.current !== 0) {
        cancelAnimationFrame(settleSnapRafRef.current);
      }
      stopSettleWatch();
    };
  }, [stopSettleWatch]);

  const handleScrollerRef = useCallback((ref: HTMLElement | Window | null) => {
    detachGestureListenersRef.current?.();
    detachGestureListenersRef.current = null;
    scrollerRef.current = ref instanceof HTMLElement ? ref : null;
    // Preserve the `#messages` selector that <MessageContextMenu> binds
    // its `contextmenu` listener to. Before the virtuoso migration the
    // outer wrapper carried this id; virtuoso owns its own scroller now
    // and we re-stamp the id on it so the right-click affordance keeps
    // working without restructuring the menu component.
    if (ref instanceof HTMLElement) {
      ref.id = 'messages';
      touchActiveRef.current = false;
      lastUpwardScrollAtRef.current = 0;
      lastScrollTopRef.current = ref.scrollTop;
      hasUserEngagedRef.current = false;
      const onPointerDown = () => {
        hasUserEngagedRef.current = true;
      };
      const onTouchStart = () => {
        hasUserEngagedRef.current = true;
        touchActiveRef.current = true;
      };
      const onTouchEnd = (e: TouchEvent) => {
        if (e.touches.length === 0) touchActiveRef.current = false;
      };
      // Wheel is tracked in addition to scroll direction so an upward intent
      // registers even when the wheel event and the height delta land before
      // the resulting scroll event does.
      const onWheel = (e: WheelEvent) => {
        hasUserEngagedRef.current = true;
        if (e.deltaY < 0) lastUpwardScrollAtRef.current = Date.now();
      };
      const onScroll = () => {
        const top = ref.scrollTop;
        if (top < lastScrollTopRef.current) {
          lastUpwardScrollAtRef.current = Date.now();
        }
        lastScrollTopRef.current = top;
        // Pre-engagement, any movement that leaves the viewport off the
        // bottom is settling churn (virtuoso initial placement and its
        // compensations move scrollTop without a height delta, so the
        // height-callback rescue alone cannot see every stranding).
        // Our own settle snap lands at the bottom and won't re-trigger.
        if (!hasUserEngagedRef.current) {
          if (ref.scrollHeight - top - ref.clientHeight > 1) {
            scheduleSettleSnap();
          }
        }
      };
      ref.addEventListener('pointerdown', onPointerDown, { passive: true });
      ref.addEventListener('touchstart', onTouchStart, { passive: true });
      ref.addEventListener('touchend', onTouchEnd, { passive: true });
      ref.addEventListener('touchcancel', onTouchEnd, { passive: true });
      ref.addEventListener('wheel', onWheel, { passive: true });
      ref.addEventListener('scroll', onScroll, { passive: true });
      detachGestureListenersRef.current = () => {
        ref.removeEventListener('pointerdown', onPointerDown);
        ref.removeEventListener('touchstart', onTouchStart);
        ref.removeEventListener('touchend', onTouchEnd);
        ref.removeEventListener('touchcancel', onTouchEnd);
        ref.removeEventListener('wheel', onWheel);
        ref.removeEventListener('scroll', onScroll);
        touchActiveRef.current = false;
      };
    }
  }, [scheduleSettleSnap]);

  // Read latest length without re-binding the callback per render.
  // `data.length === 0` is reachable when systemPrompt is present but no
  // messages have arrived; calling `scrollToIndex({ index: 'LAST' })` on
  // an empty list is library-undefined.
  const allUnitsLengthRef = useRef(allUnits.length);
  allUnitsLengthRef.current = allUnits.length;

  // Mark unread tail content when ANY tail signal fires while the user
  // is not pinned at bottom. Three independent signals cover the cases:
  //   1. `messages.length` grows — covers user/agent/system arrivals AND
  //      tool messages (which collapse into an existing agent_turn unit
  //      so `allUnits.length` would NOT change, but the visible tail
  //      content does).
  //   2. `streamingRequestId` transitions null → string — a new streaming
  //      session started; subsequent token-by-token growth is implicit
  //      while it stays non-null. We don't subscribe to per-token text
  //      changes (would break REQ-MLRU-010 streaming isolation); the
  //      session-start signal is the actionable trigger.
  //   3. `pendingMessages.length` grows — local queue gained a queued
  //      bubble (e.g. a steered message during agent activity).
  // Reset on conversation switch so unread state cannot leak across
  // navigation (MessageList stays mounted by ConversationPage and only
  // sees a prop change for `conversationId`).
  const prevMessagesLengthRef = useRef(messages.length);
  const prevStreamingRequestIdRef = useRef(streamingRequestId);
  const prevPendingLengthRef = useRef(pendingMessages.length);
  const prevConversationIdRef = useRef(conversationId);
  // Refs read by handleTotalListHeightChanged (bound via useCallback and
  // cannot see live props directly):
  //   - streamingRequestIdRef: gate height-driven unread on streaming
  //     activity so unrelated height changes (header toggle, image load
  //     in old message, late-arriving syntax highlighter) don't raise
  //     "↓ New messages".
  //   - convStateRef: also let `awaiting_sub_agents` count as active tail
  //     activity. `persist_sub_agent_results` updates the existing
  //     spawn_agents tool message via MessageUpdated rather than
  //     appending — messages.length stays the same, no stream is active,
  //     but the rendered tail genuinely grew.
  //   - lastSeenConvIdRef: tracks the conversationId at the last
  //     totalListHeightChanged callback. When it diverges from the live
  //     prop, virtuoso just re-keyed to a new conversation and emitted
  //     its first measurement for it; discard the stale baseline. Doing
  //     this inside the callback (rather than a passive useEffect) is
  //     synchronous with virtuoso's measurement timing.
  const streamingRequestIdRef = useRef(streamingRequestId);
  const convStateRef = useRef(convState);
  // Initialized to `undefined` (not `conversationId`) so the conversation-
  // switch handler in `handleTotalListHeightChanged` fires on the FIRST
  // measurement too — including initial mount. This seeds the baseline
  // (prevTotalHeightRef, prevClientHeightRef, hasSeenContentRef) without
  // scrolling, so a conversation that mounted with messages doesn't get
  // an extra forced snap on top of `initialTopMostItemIndex`.
  const lastSeenConvIdRef = useRef<string | undefined>(undefined);
  streamingRequestIdRef.current = streamingRequestId;
  convStateRef.current = convState;

  useEffect(() => {
    const conversationChanged = prevConversationIdRef.current !== conversationId;
    const prevMsgs = prevMessagesLengthRef.current;
    const prevStreamId = prevStreamingRequestIdRef.current;
    const prevPending = prevPendingLengthRef.current;
    prevConversationIdRef.current = conversationId;
    prevMessagesLengthRef.current = messages.length;
    prevStreamingRequestIdRef.current = streamingRequestId;
    prevPendingLengthRef.current = pendingMessages.length;
    if (conversationChanged) {
      setHasUnreadTailContent(false);
      // The handleTotalListHeightChanged baseline is managed by the
      // callback's own conversation-switch handler (lastSeenConvIdRef
      // check), which fires synchronously during Virtuoso's measurement
      // — before this passive useEffect runs. Resetting prevTotalHeightRef
      // here would overwrite the seeded baseline and re-introduce a
      // `prevHeight === 0` condition on the next async height delta.
      return;
    }
    if (isAtBottom) return;
    const messagesGrew = messages.length > prevMsgs;
    const streamingStarted = prevStreamId === null && streamingRequestId !== null;
    const pendingGrew = pendingMessages.length > prevPending;
    if (messagesGrew || streamingStarted || pendingGrew) {
      setHasUnreadTailContent(true);
    }
  }, [conversationId, messages.length, pendingMessages.length, streamingRequestId, isAtBottom]);

  // `followOutput={false}` disables Virtuoso's built-in auto-scroll; this
  // callback is the sole auto-follow mechanism. See REQ-MLRU-014 for the
  // full rationale (why Virtuoso's built-in handler misclassifies scroll-up
  // during streaming, and why the manual `oldFromBottom` check is correct).
  //
  // `totalListHeightChanged` fires on EVERY height delta. Use the user's
  // pre-growth scroll position vs the pre-growth bottom (captured via
  // `prevTotalHeightRef`) to fork:
  //   - within PIN threshold of old bottom → render-drift, re-snap
  //   - past PIN threshold AND height grew → user intentionally scrolled
  //     up while tail content kept arriving → mark unread so the
  //     jump-to-newest button appears (streaming-in-flight path that
  //     `messages.length` cannot catch — token growth doesn't change
  //     the array).
  //   - past PIN threshold AND height shrank → unrelated render churn;
  //     leave alone.
  //
  // Two edge cases the old `followOutput="auto"` handled and this callback
  // must replicate:
  //   - First non-empty update: when Virtuoso mounts with empty data and
  //     messages arrive later, `initialTopMostItemIndex` only controlled
  //     the mount position. Tracked via `hasSeenContentRef` (not
  //     `prevHeight === 0`, which can also fire after a baseline reset).
  //   - Viewport shrink: when clientHeight decreases (resize, panel
  //     expansion, composer growth), `oldFromBottom` uses the previous
  //     (larger) clientHeight so a pinned user stays pinned.
  const handleTotalListHeightChanged = useCallback((newHeight: number) => {
    // Detect conversation switch synchronously. virtuoso re-keys to the
    // new conversation on `key={conversationId}` change and emits its
    // first totalListHeightChanged for the new conversation during
    // mount/measurement — which can happen before any passive useEffect
    // runs. If the lastSeenConvIdRef baseline diverges from the live
    // prop, treat this as the first measurement of the new conversation
    // and reseat the prevTotalHeightRef baseline without acting on it.
    if (lastSeenConvIdRef.current !== conversationId) {
      lastSeenConvIdRef.current = conversationId;
      prevTotalHeightRef.current = newHeight;
      // Seed from the scroller so a viewport shrink before the next height
      // delta (composer growth, panel expansion right after navigation)
      // has a valid previous clientHeight to compare against. Setting 0
      // here would make the shrink handler's `clientHeight < prevClientHeight`
      // check always false (no real viewport is 0px tall), causing it to
      // use the new (smaller) height and misclassify a pinned user as
      // scrolled-up.
      const s = scrollerRef.current;
      prevClientHeightRef.current = s ? s.clientHeight : 0;
      prevScrollHeightRef.current = s ? s.scrollHeight : 0;
      // Seed from allUnitsLengthRef: if the new conversation already has
      // messages, initialTopMostItemIndex handled placement and we must
      // NOT force a snap on the next height delta. Only an empty→non-empty
      // transition (conversation was empty at switch time, content arrived
      // later) should trigger the forced snap.
      hasSeenContentRef.current = allUnitsLengthRef.current > 0;
      // Settle from the first measurement onwards rather than waiting for
      // a later stray height delta: when initialTopMostItemIndex strands
      // the mount (see hasUserEngagedRef), the stranding can be silent and
      // the next delta a second away — a visible flash of the wrong
      // position, or a permanent stranding if no delta ever comes.
      if (hasSeenContentRef.current) startSettleWatch();
      return;
    }
    const prevHeight = prevTotalHeightRef.current;
    prevTotalHeightRef.current = newHeight;
    if (allUnitsLengthRef.current === 0) return;
    const s = scrollerRef.current;
    if (!s) return;
    // First non-empty height measurement for this conversation instance:
    // initialTopMostItemIndex only controls the MOUNT position. If Virtuoso
    // mounted with empty data (fresh conversation, or cached metadata before
    // messages arrive), the first real content needs an explicit bottom
    // snap. Tracked via `hasSeenContentRef` (seeded by the conversation-
    // switch handler) rather than `prevHeight === 0`, which conflates
    // "first content" with "baseline was reset" and can re-introduce a
    // scroll-yank on a delayed height delta.
    if (!hasSeenContentRef.current) {
      hasSeenContentRef.current = true;
      prevClientHeightRef.current = s.clientHeight;
      prevScrollHeightRef.current = s.scrollHeight;
      virtuosoRef.current?.scrollToIndex({
        index: 'LAST',
        align: 'end',
        behavior: 'auto',
      });
      // Content arriving after an empty mount goes through the same
      // measurement churn as a conversation switch — watch it settle.
      startSettleWatch();
      return;
    }
    // virtuoso calls this synchronously when its internal height model
    // recomputes, before any compensatory scrollTop adjustment for the
    // new content — so scrollTop here still reflects the user's
    // pre-growth scroll position.
    //
    // Viewport shrink handling: when the scroller's clientHeight decreases
    // (browser resize, terminal/panel expansion, composer growth), the
    // oldFromBottom calculation would use the new (smaller) clientHeight,
    // making a pinned user look like they scrolled up. Use the previous
    // clientHeight to check if the user was pinned BEFORE the shrink.
    const clientHeightForPinCheck =
      s.clientHeight < prevClientHeightRef.current
        ? prevClientHeightRef.current
        : s.clientHeight;
    // Pin distance in DOM units: previous scrollHeight vs current scrollTop.
    // Not `prevHeight` (virtuoso's model total), whose estimate error on a
    // long conversation can exceed PIN_TO_BOTTOM_THRESHOLD and misclassify
    // a pinned user as scrolled-up — see prevScrollHeightRef.
    const oldFromBottom =
      prevScrollHeightRef.current - s.scrollTop - clientHeightForPinCheck;
    prevClientHeightRef.current = s.clientHeight;
    prevScrollHeightRef.current = s.scrollHeight;
    // Pre-engagement settling: see hasUserEngagedRef. Distance is
    // meaningless while virtuoso may have stranded the initial placement,
    // so keep re-snapping until the user takes over.
    if (!hasUserEngagedRef.current) {
      scheduleSettleSnap();
      return;
    }
    if (oldFromBottom <= PIN_TO_BOTTOM_THRESHOLD) {
      // An in-progress user gesture owns the viewport. Height deltas fire
      // continuously while rows mount and measure during the user's own
      // scroll-up (overscan rows above the viewport, late images, syntax
      // highlighters); re-snapping on those clobbers the gesture — on touch
      // devices it made the first ~100px of every scroll-up yank back to
      // the bottom, trapping the user there. The suppression covers the
      // finger-down drag (touchActiveRef) and the momentum/wheel phase
      // (rolling upward-scroll window). A genuinely pinned user is
      // unaffected: their scroll events are all downward or absent.
      const userOwnsViewport =
        touchActiveRef.current ||
        Date.now() - lastUpwardScrollAtRef.current < USER_SCROLL_SUPPRESS_MS;
      if (!userOwnsViewport) {
        virtuosoRef.current?.scrollToIndex({
          index: 'LAST',
          align: 'end',
          behavior: 'auto',
        });
      }
    } else if (newHeight > prevHeight) {
      // Only treat height growth as "unread tail content" when there's
      // genuine server-driven activity at the tail. Otherwise unrelated
      // growth — header toggle, image load in older message, late
      // syntax-highlighter mount on scrolled-past code block — would
      // spuriously raise the "↓ New messages" button.
      //
      // Two coarse signals indicate genuine tail activity:
      //   - active stream (token text growing inside the tail unit)
      //   - awaiting_sub_agents phase (persist_sub_agent_results updates
      //     the existing spawn_agents message via MessageUpdated;
      //     same-length but the rendered tail grows)
      // Length-grew append cases are covered by the separate
      // useEffect on messages.length / pending.
      const streamingActive = streamingRequestIdRef.current !== null;
      const subAgentsActive = convStateRef.current.type === 'awaiting_sub_agents';
      if (streamingActive || subAgentsActive) {
        setHasUnreadTailContent(true);
      } else if (import.meta.env.DEV) {
        // Height grew, user is past the pin threshold, but no genuine tail
        // activity was detected — the callback is intentionally not acting.
        // Logged only in dev to avoid noise on this hot callback in production.
        console.debug('[MessageList] height grew but not re-snapping (past threshold, no tail activity)', {
          oldFromBottom,
          heightDelta: newHeight - prevHeight,
        });
      }
    }
  }, [conversationId, scheduleSettleSnap, startSettleWatch]);

  const scrollToNewest = useCallback(() => {
    if (allUnitsLengthRef.current === 0) return;
    setHasUnreadTailContent(false);
    virtuosoRef.current?.scrollToIndex({
      index: 'LAST',
      align: 'end',
      behavior: 'auto',
    });
  }, []);

  // Conversation-nav jump + post-mount pulse. The target row is usually
  // unmounted at click time (react-virtuoso), so we can't add the highlight
  // class synchronously for a near-viewport target.
  // We stash the target unit's render-unit key and apply the pulse once the
  // row exists, after the scroll settles. `data-render-unit-key` is stamped on
  // every virtuoso row wrapper (see `itemContent`).
  const pendingPulseKeyRef = useRef<string | null>(null);
  const activeJumpKeyRef = useRef<string | null>(null);
  const jumpRetryTimersRef = useRef<number[]>([]);
  const pulseTimersRef = useRef<number[]>([]);

  const clearJumpRetryTimers = useCallback(() => {
    jumpRetryTimersRef.current.forEach((t) => clearTimeout(t));
    jumpRetryTimersRef.current.length = 0;
  }, []);

  const findRowByKey = useCallback((key: string): Element | null => {
    const scroller = scrollerRef.current;
    if (!scroller) return null;
    // CSS.escape may be absent (or throw on a pathological key) in some
    // environments; guard it so a missing escape degrades to "no pulse"
    // rather than throwing out of the timer callback (matches FileTree).
    try {
      return scroller.querySelector(`[data-render-unit-key="${CSS.escape(key)}"]`);
    } catch {
      return null;
    }
  }, []);

  const correctPendingJumpOffset = useCallback((key: string) => {
    const scroller = scrollerRef.current;
    if (!scroller) return;
    const row = findRowByKey(key);
    const target = row?.querySelector('.message') ?? row;
    if (target) ensureTargetTopVisible(target, scroller);
  }, [findRowByKey]);

  const applyPendingPulse = useCallback(() => {
    const key = pendingPulseKeyRef.current;
    if (key === null) return;
    const row = findRowByKey(key);
    // The pulse styling lives on `.message`; fall back to the row wrapper if a
    // unit kind renders without a `.message` element (skill/system don't, but
    // chapters only target user/agent units which do).
    const target = row?.querySelector('.message') ?? row;
    if (!target) return;
    pendingPulseKeyRef.current = null;
    target.classList.add('jump-highlight');
    const t = window.setTimeout(() => {
      target.classList.remove('jump-highlight');
    }, 1500);
    pulseTimersRef.current.push(t);
  }, [findRowByKey]);

  useEffect(() => {
    const pulseTimers = pulseTimersRef.current;
    const jumpRetryTimers = jumpRetryTimersRef.current;
    return () => {
      pulseTimers.forEach((t) => clearTimeout(t));
      jumpRetryTimers.forEach((t) => clearTimeout(t));
    };
  }, []);

  const scrollToUnitIndex = useCallback((unitIndex: number) => {
    const unit = historicalUnits[unitIndex];
    if (!unit) return;
    // A nav jump is user engagement even though it never touches the
    // scroller: without this, a pre-engagement height delta would snap the
    // viewport back to the bottom and clobber the jump.
    hasUserEngagedRef.current = true;
    clearJumpRetryTimers();
    activeJumpKeyRef.current = unit.key;
    pendingPulseKeyRef.current = unit.key;
    virtuosoRef.current?.scrollToIndex({
      index: unitIndex,
      align: 'center',
      behavior: 'smooth',
    });
    // The row mounts during the smooth scroll; querying immediately would miss
    // it. Retry as the scroll progresses so the pulse lands even on a long jump
    // and the final mounted position is nudged below the nav strip if needed.
    [120, 320, 600].forEach((delay) => {
      const t = window.setTimeout(() => {
        if (activeJumpKeyRef.current !== unit.key) return;
        correctPendingJumpOffset(unit.key);
        applyPendingPulse();
      }, delay);
      jumpRetryTimersRef.current.push(t);
    });
  }, [historicalUnits, clearJumpRetryTimers, correctPendingJumpOffset, applyPendingPulse]);

  useImperativeHandle(ref, () => ({ scrollToUnitIndex }), [scrollToUnitIndex]);

  const handleRangeChanged = useCallback((range: ListRange) => {
    onVisibleRangeChange?.(range);
  }, [onVisibleRangeChange]);

  const toggleSystemPrompt = useCallback(() => {
    setSystemPromptExpanded((v) => !v);
  }, []);

  // Per-conversation data for the stable Virtuoso slot component types
  // (`VIRTUOSO_COMPONENTS`). Only its *reference* changes when expansion
  // toggles — the slot types do not, so no slot remount / list-height recompute.
  const virtuosoContext = useMemo<MessageListContext>(
    () => ({ systemPrompt, systemPromptExpanded, toggleSystemPrompt }),
    [systemPrompt, systemPromptExpanded, toggleSystemPrompt],
  );

  const itemContent = useCallback(
    (_index: number, unit: RenderUnit) => (
      <div className="virtuoso-row" data-render-unit-key={unit.key}>
        {renderUnit(unit, slug, onOpenFile, filePathRootDir, onRetry, onCancelSteering, workScopeKey, activeToolUseId)}
      </div>
    ),
    [slug, onOpenFile, filePathRootDir, onRetry, onCancelSteering, workScopeKey, activeToolUseId],
  );

  const computeItemKey = useCallback(
    (_index: number, unit: RenderUnit) => unit.key,
    [],
  );

  return (
    <main id="main-area" className="chat-main-area">
      <section id="chat-view" className="view active">
        <Virtuoso
          key={conversationId ?? '__empty__'}
          ref={virtuosoRef}
          scrollerRef={handleScrollerRef}
          data={allUnits}
          context={virtuosoContext}
          itemContent={itemContent}
          computeItemKey={computeItemKey}
          followOutput={false}
          atBottomThreshold={PIN_TO_BOTTOM_THRESHOLD}
          atBottomStateChange={handleAtBottomStateChange}
          totalListHeightChanged={handleTotalListHeightChanged}
          rangeChanged={handleRangeChanged}
          // `'LAST'` is library-defined only when `data` has at least
          // one item. When systemPrompt-only renders with empty data,
          // omit this prop entirely — virtuoso's default (no initial
          // index) is correct for that case. Index 0 would target a
          // data item that doesn't exist (the Header slot is not a
          // data item).
          {...(allUnits.length > 0
            ? { initialTopMostItemIndex: { index: 'LAST' as const, align: 'end' as const } }
            : {})}
          alignToBottom
          increaseViewportBy={{ top: 600, bottom: 600 }}
          components={VIRTUOSO_COMPONENTS}
          className="message-virtuoso"
        />
      </section>
      {!isEmpty && !isAtBottom && hasUnreadTailContent && (
        <button className="jump-to-newest" onClick={scrollToNewest}>
          ↓ New messages
        </button>
      )}
      <FilePathContextMenu />
      <MessageContextMenu messages={messages} />
    </main>
  );
}

export const MessageList = memo(forwardRef(MessageListImpl));
