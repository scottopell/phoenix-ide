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
  buildHistoricalUnits,
  buildTailUnits,
  type HistoricalUnit,
  type TailUnit,
  type RenderUnit,
} from '../conversation/renderUnits';
import {
  buildConversationChapters,
  type Chapter,
} from '../conversation/conversationChapters';
import {
  PIN_TO_BOTTOM_THRESHOLD,
  SETTLE_WATCH_INTERVAL_MS,
  initialScrollMachineState,
  reduceScrollMachine,
  type ScrollEffect,
  type ScrollEvent,
  type ScrollSnapshot,
  type TailActivity,
} from '../conversation/scrollMachine';
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
  enableMessageSidepanel?: boolean | undefined;
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
  isLatestAgentMessage: boolean,
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
          forceExpandedText={isLatestAgentMessage}
          isLatestAgentMessage={isLatestAgentMessage}
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
  filePathRootDir: string | undefined,
): JSX.Element | null {
  switch (unit.kind) {
    case 'sub_agent_status':
      return <SubAgentStatus stateData={unit.state} />;
    case 'streaming_agent':
      if (!slug) return null;
      return (
        <RenderProfiler id="StreamingMessage">
          <StreamingMessage slug={slug} isFirstInTurn={unit.isFirstInTurn} rootDir={filePathRootDir} />
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
  isLatestAgentMessage: boolean,
): JSX.Element | null {
  if (
    unit.kind === 'sub_agent_status' ||
    unit.kind === 'streaming_agent'
  ) {
    return renderTailUnit(unit, slug, filePathRootDir);
  }
  return renderHistoricalUnit(unit, onOpenFile, filePathRootDir, onRetry, onCancelSteering, workScopeKey, activeToolUseId, isLatestAgentMessage);
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
  enableMessageSidepanel = true,
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
  const scrollMachineRef = useRef(initialScrollMachineState());

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

  // Split memos at the transform's natural boundary: historical units must
  // NOT rebuild on conversation state ticks. A rebuild gives every unit a
  // fresh object and toolResultsByUseId Map identity, which defeats
  // AgentMessage's memo() and re-renders every mounted row on every state
  // transition (sending → streaming → tool_executing → …) for zero visual
  // change.
  const { historicalUnits, endsInAgentRun } = useMemo(
    () => buildHistoricalUnits({ messages, pendingMessages }),
    [messages, pendingMessages],
  );
  const tailUnits = useMemo(
    () => buildTailUnits({ convState, streamingHandle, endsInAgentRun }),
    [convState, streamingHandle, endsInAgentRun],
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
    dispatchScrollEventRef.current({ type: 'atBottomChanged', atBottom });
  }, []);

  const detachGestureListenersRef = useRef<(() => void) | null>(null);
  const settleSnapRafRef = useRef(0);
  const settleWatchTimerRef = useRef(0);
  const dispatchScrollEventRef = useRef<(event: ScrollEvent) => void>(() => {});

  const readScrollSnapshot = useCallback((): ScrollSnapshot | null => {
    const s = scrollerRef.current;
    return s ? { scrollHeight: s.scrollHeight, scrollTop: s.scrollTop, clientHeight: s.clientHeight } : null;
  }, []);

  const scheduleDomBottomWrite = useCallback(() => {
    if (settleSnapRafRef.current !== 0) return;
    settleSnapRafRef.current = requestAnimationFrame(() => {
      settleSnapRafRef.current = 0;
      dispatchScrollEventRef.current({ type: 'settleTick', snapshot: readScrollSnapshot(), nowMs: Date.now() });
    });
  }, [readScrollSnapshot]);

  const stopSettleWatch = useCallback(() => {
    if (settleWatchTimerRef.current !== 0) {
      clearInterval(settleWatchTimerRef.current);
      settleWatchTimerRef.current = 0;
    }
  }, []);

  const startSettleWatch = useCallback(() => {
    scheduleDomBottomWrite();
    if (settleWatchTimerRef.current !== 0) return;
    settleWatchTimerRef.current = window.setInterval(() => {
      dispatchScrollEventRef.current({ type: 'settleTick', snapshot: readScrollSnapshot(), nowMs: Date.now() });
    }, SETTLE_WATCH_INTERVAL_MS);
  }, [readScrollSnapshot, scheduleDomBottomWrite]);

  const applyScrollEffects = useCallback((effects: ScrollEffect[]) => {
    for (const effect of effects) {
      switch (effect.type) {
        case 'snapToLastIndex':
          virtuosoRef.current?.scrollToIndex({ index: 'LAST', align: 'end', behavior: 'auto' });
          break;
        case 'scheduleDomBottomWrite':
          scheduleDomBottomWrite();
          break;
        case 'writeDomBottom': {
          const s = scrollerRef.current;
          if (s) s.scrollTop = s.scrollHeight;
          break;
        }
        case 'startSettleWatch':
          startSettleWatch();
          break;
        case 'stopSettleWatch':
          stopSettleWatch();
          break;
        case 'showUnread':
          setHasUnreadTailContent(true);
          break;
        case 'clearUnread':
          setHasUnreadTailContent(false);
          break;
        case 'debugIgnoredGrowth':
          if (import.meta.env.DEV) {
            console.debug('[MessageList] height grew but not re-snapping (past threshold, no tail activity)', {
              oldFromBottom: effect.oldFromBottom,
              heightDelta: effect.heightDelta,
            });
          }
          break;
      }
    }
  }, [scheduleDomBottomWrite, startSettleWatch, stopSettleWatch]);

  const dispatchScrollEvent = useCallback((event: ScrollEvent) => {
    const next = reduceScrollMachine(scrollMachineRef.current, event);
    scrollMachineRef.current = next.state;
    applyScrollEffects(next.effects);
  }, [applyScrollEffects]);
  dispatchScrollEventRef.current = dispatchScrollEvent;

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
    if (ref instanceof HTMLElement) {
      ref.id = 'messages';
      dispatchScrollEvent({
        type: 'scrollerAttached',
        snapshot: { scrollHeight: ref.scrollHeight, scrollTop: ref.scrollTop, clientHeight: ref.clientHeight },
      });
      const onPointerDown = () => dispatchScrollEvent({ type: 'pointerDown' });
      const onTouchStart = () => dispatchScrollEvent({ type: 'touchStart', nowMs: Date.now() });
      const onTouchEnd = (e: TouchEvent) => dispatchScrollEvent({ type: 'touchEnd', remainingTouches: e.touches.length, nowMs: Date.now() });
      const onWheel = (e: WheelEvent) => dispatchScrollEvent({ type: 'wheel', deltaY: e.deltaY, nowMs: Date.now() });
      const onScroll = () => dispatchScrollEvent({
        type: 'scroll',
        snapshot: { scrollHeight: ref.scrollHeight, scrollTop: ref.scrollTop, clientHeight: ref.clientHeight },
        nowMs: Date.now(),
      });
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
      };
    }
  }, [dispatchScrollEvent]);

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
  //     activity so unrelated height changes don't raise "↓ New messages".
  //   - convStateRef: also let `awaiting_sub_agents` count as active tail
  //     activity when existing tool output grows without a new message.
  const streamingRequestIdRef = useRef(streamingRequestId);
  const convStateRef = useRef(convState);
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

  const handleTotalListHeightChanged = useCallback((newHeight: number) => {
    const tailActivity: TailActivity =
      streamingRequestIdRef.current !== null || convStateRef.current.type === 'awaiting_sub_agents'
        ? 'active'
        : 'none';
    dispatchScrollEvent({
      type: 'totalHeightChanged',
      conversationId,
      totalHeight: newHeight,
      unitCount: allUnitsLengthRef.current,
      snapshot: readScrollSnapshot(),
      tailActivity,
      nowMs: Date.now(),
    });
  }, [conversationId, dispatchScrollEvent, readScrollSnapshot]);

  const scrollToNewest = useCallback(() => {
    dispatchScrollEvent({ type: 'jumpToNewestClicked', unitCount: allUnitsLengthRef.current });
  }, [dispatchScrollEvent]);

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
    dispatchScrollEvent({ type: 'navJump' });
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
  }, [historicalUnits, clearJumpRetryTimers, correctPendingJumpOffset, applyPendingPulse, dispatchScrollEvent]);

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

  const latestAgentKey = useMemo(() => {
    for (let i = historicalUnits.length - 1; i >= 0; i -= 1) {
      const unit = historicalUnits[i];
      if (unit?.kind === 'agent_turn') return unit.key;
    }
    return null;
  }, [historicalUnits]);

  const itemContent = useCallback(
    (_index: number, unit: RenderUnit) => (
      <div className="virtuoso-row" data-render-unit-key={unit.key}>
        {renderUnit(unit, slug, onOpenFile, filePathRootDir, onRetry, onCancelSteering, workScopeKey, activeToolUseId, unit.kind === 'agent_turn' && unit.key === latestAgentKey)}
      </div>
    ),
    [slug, onOpenFile, filePathRootDir, onRetry, onCancelSteering, workScopeKey, activeToolUseId, latestAgentKey],
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
      <MessageContextMenu messages={messages} enableMessageSidepanel={enableMessageSidepanel} />
    </main>
  );
}

export const MessageList = memo(forwardRef(MessageListImpl));
