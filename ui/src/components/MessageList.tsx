import { memo, useState, useRef, useCallback, useMemo, useEffect, forwardRef, useImperativeHandle } from 'react';
import { Virtuoso, type VirtuosoHandle, type VirtuosoProps, type ListRange } from 'react-virtuoso';
import type { Message, ConversationState } from '../api';
import type { QueuedMessage } from '../hooks';
import {
  UserMessage,
  QueuedUserMessage,
  AgentMessage,
  SubAgentStatus,
  formatMessageTime,
} from './MessageComponents';
import { StreamingMessage } from './StreamingMessage';
import { RenderProfiler } from '../dev/renderProfiler';
import { MessageContextMenu } from './MessageContextMenu';
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

function extractSkillArgs(trigger: string, name: string): string {
  return trigger.replace(new RegExp(`^/?${name}\\s*`), '').trim();
}

type OnOpenFile = ((filePath: string, modifiedLines: Set<number>, firstModifiedLine: number) => void) | undefined;

function renderHistoricalUnit(
  unit: HistoricalUnit,
  onOpenFile: OnOpenFile,
  onRetry: (localId: string) => void,
  onCancelSteering: ((localId: string) => void) | undefined,
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
      const c = unit.message.content as { name?: string; trigger?: string };
      const trigger = c.trigger || '';
      const args = extractSkillArgs(trigger, c.name || '');
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
            <div className="skill-indicator" title={`Skill invocation: loaded instructions from /${c.name || 'skill'}/SKILL.md and delivered to the agent`}>
              <span className="skill-label">skill: /{c.name || 'skill'}</span>
              {args && (
                <span className="skill-trigger">{args}</span>
              )}
            </div>
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
          <StreamingMessage slug={slug} />
        </RenderProfiler>
      );
  }
}

function renderUnit(
  unit: RenderUnit,
  slug: string | undefined,
  onOpenFile: OnOpenFile,
  onRetry: (localId: string) => void,
  onCancelSteering: ((localId: string) => void) | undefined,
): JSX.Element | null {
  if (
    unit.kind === 'sub_agent_status' ||
    unit.kind === 'streaming_agent'
  ) {
    return renderTailUnit(unit, slug);
  }
  return renderHistoricalUnit(unit, onOpenFile, onRetry, onCancelSteering);
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
  // to detect "user was pinned to the previous bottom" — see the
  // handleTotalListHeightChanged comment.
  const prevTotalHeightRef = useRef(0);

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

  const handleAtBottomStateChange = useCallback((atBottom: boolean) => {
    setIsAtBottom(atBottom);
    // Returning to bottom clears the unread badge regardless of whether
    // the user got there via scroll, click, or virtuoso's followOutput.
    if (atBottom) setHasUnreadTailContent(false);
  }, []);

  const handleScrollerRef = useCallback((ref: HTMLElement | Window | null) => {
    scrollerRef.current = ref instanceof HTMLElement ? ref : null;
    // Preserve the `#messages` selector that <MessageContextMenu> binds
    // its `contextmenu` listener to. Before the virtuoso migration the
    // outer wrapper carried this id; virtuoso owns its own scroller now
    // and we re-stamp the id on it so the right-click affordance keeps
    // working without restructuring the menu component.
    if (ref instanceof HTMLElement) {
      ref.id = 'messages';
    }
  }, []);

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
  const lastSeenConvIdRef = useRef(conversationId);
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
      // The handleTotalListHeightChanged baseline must reset alongside
      // the rest of the per-conversation state. Otherwise the first
      // callback in conversation B compares against conversation A's
      // total list height and can spuriously fire re-snap or unread.
      prevTotalHeightRef.current = 0;
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

  // virtuoso's `followOutput="auto"` only fires when `data.length` grows;
  // it doesn't re-snap when the LAST item's height changes async after
  // mount (markdown render, react-syntax-highlighter mounting, image
  // loading). That leaves the user a few hundred pixels above true bottom
  // — visually "not at the bottom" despite virtuoso's internal pin.
  //
  // `totalListHeightChanged` fires on EVERY height delta, including:
  //   - new items appended
  //   - existing items resizing (markdown render, code highlighter mount,
  //     streaming token text growth, MessageUpdated mutating existing
  //     content like spawn_agents → sub-agent results)
  //
  // Use the user's pre-growth scroll position vs the pre-growth bottom
  // (captured via `prevTotalHeightRef`) to fork:
  //   - within PIN threshold of old bottom → render-drift, re-snap
  //   - past PIN threshold AND height grew → user intentionally
  //     scrolled up while tail content kept arriving → mark unread
  //     content so the jump-to-newest button appears. This is the
  //     streaming-in-flight path that `messages.length` cannot catch
  //     (token growth doesn't change the array).
  //   - past PIN threshold AND height shrank → unrelated render churn
  //     (e.g. an item collapsed); leave alone.
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
      return;
    }
    const prevHeight = prevTotalHeightRef.current;
    prevTotalHeightRef.current = newHeight;
    if (allUnitsLengthRef.current === 0) return;
    // First non-empty render: initialTopMostItemIndex handles placement.
    if (prevHeight === 0) return;
    const s = scrollerRef.current;
    if (!s) return;
    // virtuoso calls this synchronously when its internal height model
    // recomputes, before any compensatory scrollTop adjustment for the
    // new content — so scrollTop here still reflects the user's
    // pre-growth scroll position.
    const oldFromBottom = prevHeight - s.scrollTop - s.clientHeight;
    if (oldFromBottom <= PIN_TO_BOTTOM_THRESHOLD) {
      virtuosoRef.current?.scrollToIndex({
        index: 'LAST',
        align: 'end',
        behavior: 'auto',
      });
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
      }
    }
  }, [conversationId]);

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
  // class synchronously the way the legacy near-viewport breadcrumb jump did.
  // We stash the target unit's render-unit key and apply the pulse once the
  // row exists, after the scroll settles. `data-render-unit-key` is stamped on
  // every virtuoso row wrapper (see `itemContent`).
  const pendingPulseKeyRef = useRef<string | null>(null);
  const pulseTimersRef = useRef<number[]>([]);

  const applyPendingPulse = useCallback(() => {
    const key = pendingPulseKeyRef.current;
    if (key === null) return;
    const scroller = scrollerRef.current;
    if (!scroller) return;
    const row = scroller.querySelector(
      `[data-render-unit-key="${CSS.escape(key)}"]`,
    );
    // The pulse styling lives on `.message`; fall back to the row wrapper if a
    // unit kind renders without a `.message` element (skill/system don't, but
    // chapters only target user/agent units which do).
    const target = row?.querySelector('.message') ?? row;
    if (!target) return;
    pendingPulseKeyRef.current = null;
    target.classList.add('breadcrumb-highlight');
    const t = window.setTimeout(() => {
      target.classList.remove('breadcrumb-highlight');
    }, 1500);
    pulseTimersRef.current.push(t);
  }, []);

  useEffect(() => {
    const timers = pulseTimersRef.current;
    return () => {
      timers.forEach((t) => clearTimeout(t));
    };
  }, []);

  const scrollToUnitIndex = useCallback((unitIndex: number) => {
    const unit = historicalUnits[unitIndex];
    if (!unit) return;
    pendingPulseKeyRef.current = unit.key;
    virtuosoRef.current?.scrollToIndex({
      index: unitIndex,
      align: 'center',
      behavior: 'smooth',
    });
    // The row mounts during the smooth scroll; querying immediately would miss
    // it. Retry a few times so the pulse lands even on a long jump that takes
    // a moment to settle. applyPendingPulse is a no-op once the pulse fires
    // (it clears pendingPulseKeyRef), so the extra ticks are harmless.
    [120, 320, 600].forEach((delay) => {
      const t = window.setTimeout(applyPendingPulse, delay);
      pulseTimersRef.current.push(t);
    });
  }, [historicalUnits, applyPendingPulse]);

  useImperativeHandle(ref, () => ({ scrollToUnitIndex }), [scrollToUnitIndex]);

  const handleRangeChanged = useCallback((range: ListRange) => {
    onVisibleRangeChange?.(range);
  }, [onVisibleRangeChange]);

  const toggleSystemPrompt = useCallback(() => {
    setSystemPromptExpanded((v) => !v);
  }, []);

  const SystemPromptHeaderSlot = useMemo(() => {
    if (!systemPrompt) return undefined;
    const Header = () => (
      <SystemPromptHeader
        systemPrompt={systemPrompt}
        expanded={systemPromptExpanded}
        onToggle={toggleSystemPrompt}
      />
    );
    return Header;
  }, [systemPrompt, systemPromptExpanded, toggleSystemPrompt]);

  const itemContent = useCallback(
    (_index: number, unit: RenderUnit) => (
      <div className="virtuoso-row" data-render-unit-key={unit.key}>
        {renderUnit(unit, slug, onOpenFile, onRetry, onCancelSteering)}
      </div>
    ),
    [slug, onOpenFile, onRetry, onCancelSteering],
  );

  const computeItemKey = useCallback(
    (_index: number, unit: RenderUnit) => unit.key,
    [],
  );

  // Empty-state UI lives in virtuoso's `EmptyPlaceholder` slot rather than
  // a parallel branch, so that systemPrompt (rendered as virtuoso's
  // Header) stays visible alongside the "Start a conversation" affordance
  // for a freshly-opened conversation with a system prompt and no
  // messages yet. The previous parallel-branch approach hid the empty
  // state once a system prompt loaded, which surprised users opening a
  // new conversation.
  const EmptyPlaceholder = useMemo(() => {
    const Component = () => (
      <div className="empty-state">
        <div className="empty-state-icon"><MessageSquareIcon /></div>
        <p>Start a conversation</p>
      </div>
    );
    return Component;
  }, []);

  const virtuosoComponents = useMemo<NonNullable<VirtuosoProps<RenderUnit, unknown>['components']>>(() => {
    const components: NonNullable<VirtuosoProps<RenderUnit, unknown>['components']> = {
      EmptyPlaceholder,
    };
    if (SystemPromptHeaderSlot) {
      components.Header = SystemPromptHeaderSlot;
    }
    return components;
  }, [EmptyPlaceholder, SystemPromptHeaderSlot]);

  return (
    <main id="main-area">
      <section id="chat-view" className="view active">
        <Virtuoso
          key={conversationId ?? '__empty__'}
          ref={virtuosoRef}
          scrollerRef={handleScrollerRef}
          data={allUnits}
          itemContent={itemContent}
          computeItemKey={computeItemKey}
          followOutput="auto"
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
          components={virtuosoComponents}
          className="message-virtuoso"
        />
      </section>
      {!isEmpty && !isAtBottom && hasUnreadTailContent && (
        <button className="jump-to-newest" onClick={scrollToNewest}>
          ↓ New messages
        </button>
      )}
      <MessageContextMenu messages={messages} />
    </main>
  );
}

export const MessageList = memo(forwardRef(MessageListImpl));
