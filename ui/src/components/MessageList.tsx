import { memo, useState, useEffect, useRef, useCallback, useMemo } from 'react';
import { flushSync } from 'react-dom';
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
import { MessageContextMenu } from './MessageContextMenu';
import { useBottomAnchoredWindow } from '../hooks/useBottomAnchoredWindow';
import { useUnitHeightCache } from '../conversation/unitHeightCache';
import { useStreamingStartedAt } from '../conversation/useConversationAtom';
import { useUnitHeightObserver, type UnitHeightObserver } from '../hooks/useUnitHeightObserver';
import {
  buildRenderUnits,
  type HistoricalUnit,
  type TailUnit,
} from '../conversation/renderUnits';

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
  /**
   * Messages the client has queued that have NOT yet appeared in `messages`
   * (by `message_id == localId` match). The parent computes this as a pure
   * derivation of the queue and `atom.messages`, so "sending" is implicit —
   * presence in this list means "still waiting for the server echo."
   * Failed messages are NOT included here; they render in InputArea.
   */
  pendingMessages: QueuedMessage[];
  convState: ConversationState;
  onRetry: (localId: string) => void;
  /** Called when the user presses the × button on a `steering_queued` bubble. */
  onCancelSteering?: ((localId: string) => void) | undefined;
  onOpenFile: ((filePath: string, modifiedLines: Set<number>, firstModifiedLine: number) => void) | undefined;
  systemPrompt?: string | undefined;
  /** Backend conversation UUID (atom.conversationId). Used as the parent
   *  prop tying several pieces of internal state to a single
   *  conversation. */
  conversationId?: string | undefined;
  /** URL slug — the key the conversation store is keyed by. Needed for
   *  both the streaming-active subscription (via useStreamingStartedAt)
   *  and the streaming-buffer subscription inside <StreamingMessage>.
   *  May be undefined briefly during route transitions; when undefined,
   *  no streaming tail unit is emitted. */
  slug?: string | undefined;
}

// Threshold in pixels - if user is within this distance of bottom, consider them "pinned"
const SCROLL_THRESHOLD = 100;

// Extracts the arguments portion of a skill trigger string, stripping the leading skill name.
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
      return <UserMessage key={unit.key} message={unit.message} />;
    case 'pending_user':
      return (
        <QueuedUserMessage
          key={unit.key}
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
        <div key={unit.key} className="message user" data-sequence-id={unit.message.sequence_id}>
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
          key={unit.key}
          message={unit.agent}
          toolResults={unit.toolResultsByUseId}
          onOpenFile={onOpenFile}
          isFirstInTurn={unit.isFirstInTurn}
        />
      );
    case 'system': {
      // buildRenderUnits skips empty-text system messages, so this branch
      // always has text — but read defensively in case the contract drifts.
      const text = (unit.message.content as { text?: string })?.text;
      if (!text) return null;
      return (
        <div key={unit.key} className="system-message">
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
      return <SubAgentStatus key={unit.key} stateData={unit.state} />;
    case 'streaming_agent':
      // The slug-less case should never reach here in practice because
      // MessageList only emits the streaming_agent unit when slug is
      // set. Defensive null-return keeps the type narrow.
      if (!slug) return null;
      return <StreamingMessage key={unit.key} slug={slug} />;
  }
}

interface MessageListBodyProps {
  historicalUnits: HistoricalUnit[];
  tailUnits: TailUnit[];
  firstRenderedUnitIndex: number;
  lastRenderedUnitIndex: number;
  spacerHeight: number;
  bottomSpacerHeight: number;
  topSentinelRef: (node: HTMLDivElement | null) => void;
  bottomSentinelRef: (node: HTMLDivElement | null) => void;
  observeUnit: UnitHeightObserver['observe'];
  slug: string | undefined;
  onRetry: (localId: string) => void;
  onCancelSteering?: ((localId: string) => void) | undefined;
  onOpenFile: OnOpenFile;
}

/**
 * Memoized subtree holding the slice over historical render units.
 * The parent <MessageList> is itself memo'd so token churn doesn't
 * even reach this layer; this memo is the inner belt-and-suspenders
 * boundary that keeps the historical render path stable when other
 * props (e.g. callbacks) churn at the parent.
 *
 * Each rendered unit is wrapped in a `<div>` that owns the
 * ResizeObserver-attaching ref callback.
 */
const MessageListBody = memo(function MessageListBody({
  historicalUnits,
  tailUnits,
  firstRenderedUnitIndex,
  lastRenderedUnitIndex,
  spacerHeight,
  bottomSpacerHeight,
  topSentinelRef,
  bottomSentinelRef,
  observeUnit,
  slug,
  onRetry,
  onCancelSteering,
  onOpenFile,
}: MessageListBodyProps) {
  return (
    <>
      {firstRenderedUnitIndex > 0 && (
        <div
          className="message-collapsed-spacer"
          style={{ height: spacerHeight }}
          aria-hidden="true"
        />
      )}
      <div ref={topSentinelRef} aria-hidden="true" />
      {historicalUnits
        .slice(firstRenderedUnitIndex, lastRenderedUnitIndex)
        .map((unit) => (
          <div
            key={unit.key}
            ref={observeUnit(unit)}
            data-render-unit-key={unit.key}
          >
            {renderHistoricalUnit(unit, onOpenFile, onRetry, onCancelSteering)}
          </div>
        ))}
      {lastRenderedUnitIndex < historicalUnits.length && (
        <div ref={bottomSentinelRef} aria-hidden="true" />
      )}
      {lastRenderedUnitIndex < historicalUnits.length && (
        <div
          className="message-collapsed-spacer"
          style={{ height: bottomSpacerHeight }}
          aria-hidden="true"
        />
      )}
      {tailUnits.map((unit) => (
        <div key={unit.key} data-render-unit-key={unit.key}>
          {renderTailUnit(unit, slug)}
        </div>
      ))}
    </>
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
}: MessageListProps) {
  const [systemPromptExpanded, setSystemPromptExpanded] = useState(false);
  const [jumpToNewestState, setJumpToNewestState] = useState<{
    conversationId: string | undefined;
    visible: boolean;
  }>({ conversationId, visible: false });
  const mainRef = useRef<HTMLElement>(null);
  const messagesRef = useRef<HTMLDivElement>(null);
  const isPinnedToBottom = useRef(true); // Start pinned to bottom
  const lastScrollTop = useRef(0);
  const prevMessagesHeight = useRef(0);
  // Tracks message count between renders so the ResizeObserver knows whether a
  // new message arrived (fix: height can *decrease* when streaming clears and the
  // finalized message is shorter, which would otherwise suppress auto-scroll).
  const prevMessageCountRef = useRef(messages.length);
  // 'force' = new system message → scroll regardless of pin state.
  // 'soft'  = new non-system message → scroll only if pinned.
  // 'none'  = no new message this render.
  const scrollTriggerRef = useRef<'none' | 'soft' | 'force'>('none');
  const lastConversationIdRef = useRef<string | undefined>(conversationId);

  if (lastConversationIdRef.current !== conversationId) {
    lastConversationIdRef.current = conversationId;
    prevMessagesHeight.current = 0;
    prevMessageCountRef.current = messages.length;
    scrollTriggerRef.current = 'none';
    lastScrollTop.current = mainRef.current?.scrollTop ?? 0;
    isPinnedToBottom.current = true;
  }

  // Streaming session identity: useStreamingStartedAt subscribes to
  // `streamingBuffer?.startedAt` for this slug and re-renders only when
  // a session starts / ends / restarts (not per-token). The actual
  // buffer text is subscribed inside <StreamingMessage> via
  // useStreamingBuffer.
  //
  // Including startedAt in the key forces React to remount
  // <StreamingMessage> across back-to-back sessions (e.g., when
  // sse_message and sse_token land in the same React batch and the
  // streaming-active boolean never observes false between sessions).
  // Without it, the leaf's pendingText / displayText state from
  // session N would briefly bleed into session N+1's first frame.
  const streamingStartedAt = useStreamingStartedAt(slug);
  const streamingHandle = useMemo(
    () => (streamingStartedAt !== null && slug
      ? { key: `streaming-${slug}-${streamingStartedAt}` }
      : null),
    [streamingStartedAt, slug],
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

  const heightCache = useUnitHeightCache(conversationId);
  const unitObserver = useUnitHeightObserver(heightCache);

  const {
    firstRenderedUnitIndex,
    lastRenderedUnitIndex,
    spacerHeight,
    bottomSpacerHeight,
    topSentinelRef,
    bottomSentinelRef,
    resetToBottom,
  } = useBottomAnchoredWindow({
    historicalUnits,
    conversationId,
    scrollRootRef: mainRef,
    heightCache,
  });

  if (messages.length > prevMessageCountRef.current) {
    const newMsgs = messages.slice(prevMessageCountRef.current);
    const hasSystem = newMsgs.some(m => (m.message_type || m.type) === 'system');
    scrollTriggerRef.current = hasSystem ? 'force' : 'soft';
    prevMessageCountRef.current = messages.length;
  }

  const showJumpToNewest = jumpToNewestState.conversationId === conversationId
    ? jumpToNewestState.visible
    : false;

  // Check if user is near bottom of scroll
  const checkIfPinnedToBottom = useCallback(() => {
    const el = mainRef.current;
    if (!el) return true;
    const distanceFromBottom = el.scrollHeight - el.scrollTop - el.clientHeight;
    return distanceFromBottom <= SCROLL_THRESHOLD;
  }, []);

  // Track pin-to-bottom on scroll, and clear the jump-to-newest button
  // when the user returns to the bottom.
  const handleScroll = useCallback(() => {
    isPinnedToBottom.current = checkIfPinnedToBottom();
    const el = mainRef.current;
    if (el) {
      lastScrollTop.current = el.scrollTop;
    }
    if (isPinnedToBottom.current) {
      setJumpToNewestState((prev) => (
        prev.conversationId === conversationId && !prev.visible
          ? prev
          : { conversationId, visible: false }
      ));
    }
  }, [checkIfPinnedToBottom, conversationId]);

  // Scroll to bottom helper
  const scrollToBottom = useCallback(() => {
    if (mainRef.current) {
      mainRef.current.scrollTop = mainRef.current.scrollHeight;
      lastScrollTop.current = mainRef.current.scrollTop;
    }
  }, []);

  // Single ResizeObserver drives all auto-scroll.
  // Fires after layout is complete (unlike rAF which fires before paint), so
  // scrollHeight is always the settled value — no mid-render jumps.
  // Triggers on any content growth: streaming tokens, new messages, new tool blocks.
  // Also fires on net-zero or negative height changes when a new message arrived
  // (streaming clears + finalized message renders = possible height decrease).
  useEffect(() => {
    const messagesEl = messagesRef.current;
    if (!messagesEl) return;

    const observer = new ResizeObserver((entries) => {
      for (const entry of entries) {
        const newHeight = entry.contentRect.height;
        const trigger = scrollTriggerRef.current;
        scrollTriggerRef.current = 'none';

        const heightGrew = newHeight > prevMessagesHeight.current;
        const shouldAct = heightGrew || trigger !== 'none';

        if (shouldAct) {
          if (isPinnedToBottom.current || trigger === 'force') {
            mainRef.current!.scrollTop = mainRef.current!.scrollHeight;
            lastScrollTop.current = mainRef.current!.scrollTop;
            if (trigger === 'force') isPinnedToBottom.current = true;
          } else {
            setJumpToNewestState((prev) => (
              prev.conversationId === conversationId && prev.visible
                ? prev
                : { conversationId, visible: true }
            ));
          }
        }
        prevMessagesHeight.current = newHeight;
      }
    });

    observer.observe(messagesEl);
    return () => observer.disconnect();
  }, [conversationId]);

  // isEmpty reflects "nothing for MessageListBody to render". Includes
  // tail units (sub_agent_status, streaming_agent) so the empty-state
  // placeholder doesn't hide an active streaming buffer or sub-agent
  // status block when historical messages are still empty.
  const isEmpty = historicalUnits.length === 0 && tailUnits.length === 0;

  return (
    <main id="main-area" ref={mainRef} onScroll={handleScroll}>
      <section id="chat-view" className="view active">
        <div id="messages" ref={messagesRef}>
          {systemPrompt && (
            <div className={`system-prompt-block${systemPromptExpanded ? ' expanded' : ''}`}>
              <div
                className="system-prompt-header"
                onClick={() => setSystemPromptExpanded((v) => !v)}
              >
                <span className="system-prompt-label">System prompt</span>
                <span className="system-prompt-toggle">
                  {systemPromptExpanded ? <ChevronDown /> : <ChevronRight />}
                  {systemPromptExpanded ? ' hide' : ' show'}
                </span>
              </div>
              {systemPromptExpanded && (
                <pre className="system-prompt-content">{systemPrompt}</pre>
              )}
            </div>
          )}
          {isEmpty ? (
            <div className="empty-state">
              <div className="empty-state-icon"><MessageSquareIcon /></div>
              <p>Start a conversation</p>
            </div>
          ) : (
            <MessageListBody
              historicalUnits={historicalUnits}
              tailUnits={tailUnits}
              firstRenderedUnitIndex={firstRenderedUnitIndex}
              lastRenderedUnitIndex={lastRenderedUnitIndex}
              spacerHeight={spacerHeight}
              bottomSpacerHeight={bottomSpacerHeight}
              topSentinelRef={topSentinelRef}
              bottomSentinelRef={bottomSentinelRef}
              observeUnit={unitObserver.observe}
              slug={slug}
              onRetry={onRetry}
              onCancelSteering={onCancelSteering}
              onOpenFile={onOpenFile}
            />
          )}
          {/* Streaming text — now rendered via the streaming_agent TailUnit
              inside <MessageListBody>. <StreamingMessage> subscribes to the
              buffer via useStreamingBuffer(slug), so per-token mutations
              re-render only the leaf — MessageList and MessageListBody are
              untouched by token churn (REQ-MLRU-010). */}
        </div>
      </section>
      {showJumpToNewest && (
        <button
          className="jump-to-newest"
          onClick={() => {
            flushSync(resetToBottom);
            scrollToBottom();
            setJumpToNewestState((prev) => (
              prev.conversationId === conversationId && !prev.visible
                ? prev
                : { conversationId, visible: false }
            ));
          }}
        >
          ↓ New messages
        </button>
      )}
      <MessageContextMenu messages={messages} />
    </main>
  );
}

/**
 * memo-wrapped export. The parent `<ConversationPage>` re-renders on
 * every token (it consumes the whole atom via useConversationAtom); this
 * memo boundary stops that re-render from propagating here, because the
 * props are reference-stable across token mutations:
 *   - messages / pendingMessages / convState come from atom slices that
 *     don't change on tokens
 *   - callbacks (onRetry / onCancelSteering / onOpenFile) are
 *     useCallback'd at the parent
 *   - slug / conversationId are URL/atom-derived strings
 *   - systemPrompt is a string from the atom that doesn't mutate per token
 *
 * Internally, MessageList subscribes to `useStreamingStartedAt(slug)` —
 * a primitive (number | null) that stays Object.is-stable through every
 * token within a session, so the subscription notification on each token
 * does not trigger a re-render. The buffer text itself is consumed by
 * <StreamingMessage> via useStreamingBuffer(slug); only that leaf
 * re-renders per token.
 */
export const MessageList = memo(MessageListImpl);

