import { memo, useState, useEffect, useLayoutEffect, useRef, useCallback, useMemo, type RefObject } from 'react';
import type { Message, ConversationState } from '../api';
import type { QueuedMessage } from '../hooks';
import type { StreamingBuffer } from '../conversation/atom';
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
  conversationId?: string | undefined;
  streamingBuffer?: StreamingBuffer | null;
}

// Threshold in pixels - if user is within this distance of bottom, consider them "pinned"
const SCROLL_THRESHOLD = 100;

const SCROLL_KEY_PREFIX = 'phoenix:scroll:';
const MSGCOUNT_KEY_PREFIX = 'phoenix:msgcount:';

// Extracts the arguments portion of a skill trigger string, stripping the leading skill name.
function extractSkillArgs(trigger: string, name: string): string {
  return trigger.replace(new RegExp(`^/?${name}\\s*`), '').trim();
}

type OnOpenFile = ((filePath: string, modifiedLines: Set<number>, firstModifiedLine: number) => void) | undefined;

function renderHistoricalUnit(unit: HistoricalUnit, onOpenFile: OnOpenFile): JSX.Element | null {
  switch (unit.kind) {
    case 'user':
      return <UserMessage key={unit.key} message={unit.message} />;
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
  onRetry: (localId: string) => void,
  onCancelSteering: ((localId: string) => void) | undefined,
): JSX.Element | null {
  switch (unit.kind) {
    case 'pending_user':
      return (
        <QueuedUserMessage
          key={unit.key}
          message={unit.message}
          onRetry={onRetry}
          onCancelSteering={onCancelSteering}
        />
      );
    case 'sub_agent_status':
      return <SubAgentStatus key={unit.key} stateData={unit.state} />;
    case 'streaming_agent':
      // The streaming view is rendered as a sibling of <MessageListBody>
      // (see <StreamingMessage> below). Step 5 will move the subscription
      // into the leaf so this case becomes <StreamingMessage key={unit.key} />.
      return null;
  }
}

interface MessageListBodyProps {
  historicalUnits: HistoricalUnit[];
  tailUnits: TailUnit[];
  firstRenderedUnitIndex: number;
  spacerHeight: number;
  topSentinelRef: RefObject<HTMLDivElement>;
  onRetry: (localId: string) => void;
  onCancelSteering?: ((localId: string) => void) | undefined;
  onOpenFile: OnOpenFile;
}

/**
 * Memoized subtree holding the slice over historical render units.
 * React.memo's shallow prop compare skips re-render when historicalUnits,
 * tailUnits, and the window outputs are reference-stable — which they are
 * across streaming token updates (the parent's streamingBuffer prop
 * changes, but buildRenderUnits is useMemo'd over messages so the unit
 * arrays don't reallocate per token).
 */
const MessageListBody = memo(function MessageListBody({
  historicalUnits,
  tailUnits,
  firstRenderedUnitIndex,
  spacerHeight,
  topSentinelRef,
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
        .slice(firstRenderedUnitIndex)
        .map((unit) => renderHistoricalUnit(unit, onOpenFile))}
      {tailUnits.map((unit) => renderTailUnit(unit, onRetry, onCancelSteering))}
    </>
  );
});

export function MessageList({
  messages,
  pendingMessages,
  convState,
  onRetry,
  onCancelSteering,
  onOpenFile,
  systemPrompt,
  conversationId,
  streamingBuffer,
}: MessageListProps) {
  const [systemPromptExpanded, setSystemPromptExpanded] = useState(false);
  const [jumpToNewestState, setJumpToNewestState] = useState<{
    conversationId: string | undefined;
    visible: boolean;
  }>({ conversationId, visible: false });
  const mainRef = useRef<HTMLElement>(null);
  const messagesRef = useRef<HTMLDivElement>(null);
  const isPinnedToBottom = useRef(true); // Start pinned to bottom
  const lastRestoredConversationId = useRef<string | undefined>(undefined);
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

  // Saved scroll pixel for REQ-CONV-013, read synchronously and memoized per
  // conversation so the bottom-anchored window can widen far enough that the
  // restored scrollTop lands inside REAL rendered content (not the estimated
  // spacer). Recomputed on conversationId change, same render-time pattern as
  // the rest of this component.
  const savedScrollLookupRef = useRef<{ id: string | undefined; pos: number | null }>({
    id: undefined,
    pos: null,
  });
  if (savedScrollLookupRef.current.id !== conversationId) {
    let pos: number | null = null;
    if (conversationId) {
      try {
        const raw = localStorage.getItem(`${SCROLL_KEY_PREFIX}${conversationId}`);
        pos = raw !== null ? parseInt(raw, 10) : null;
        if (pos !== null && Number.isNaN(pos)) pos = null;
      } catch { pos = null; }
    }
    savedScrollLookupRef.current = { id: conversationId, pos };
  }

  const { historicalUnits, tailUnits } = useMemo(
    () => buildRenderUnits({
      messages,
      pendingMessages,
      convState,
      // Streaming is rendered as a sibling for now; step 5 wires the
      // streaming-buffer atom into a TailUnit + leaf subscription.
      streamingHandle: null,
    }),
    [messages, pendingMessages, convState],
  );

  const {
    firstRenderedUnitIndex,
    spacerHeight,
    topSentinelRef,
  } = useBottomAnchoredWindow({
    historicalUnits,
    conversationId,
    scrollRootRef: mainRef,
    // Unit-anchor restore is wired in step 4; for now bottom-pin on mount
    // and the existing scrollTop-based restore below handles continuity.
    savedAnchor: null,
  });
  // Suppress unused-variable warning until step 4 wires this through the
  // unit-anchor save/read. Held in the lookup so step 4 only needs to
  // change the call site, not re-introduce the read.
  void savedScrollLookupRef;

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

  // Handle scroll events to track if user is pinned to bottom
  const handleScroll = useCallback(() => {
    isPinnedToBottom.current = checkIfPinnedToBottom();
    const el = mainRef.current;
    if (el) lastScrollTop.current = el.scrollTop;
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

  // Save scroll position on unmount / visibility change (REQ-CONV-013)
  useEffect(() => {
    if (!conversationId) return;
    const saveScroll = () => {
      try {
        // Use ref for scroll position — DOM element may be detached on unmount
        localStorage.setItem(`${SCROLL_KEY_PREFIX}${conversationId}`, String(lastScrollTop.current));
        localStorage.setItem(`${MSGCOUNT_KEY_PREFIX}${conversationId}`, String(messages.length));
      } catch { /* storage full - degrade gracefully */ }
    };
    const onVisChange = () => {
      if (document.visibilityState === 'hidden') saveScroll();
    };
    document.addEventListener('visibilitychange', onVisChange);
    return () => {
      document.removeEventListener('visibilitychange', onVisChange);
      saveScroll(); // save on unmount (route change)
    };
  }, [conversationId, messages.length]);

  // Restore scroll position on mount after messages render (REQ-CONV-013).
  // useLayoutEffect runs synchronously after DOM commit and before the browser
  // fires ResizeObserver, so isPinnedToBottom is correctly set before the
  // observer decides whether to auto-scroll — no rAF, no flash to bottom first.
  useLayoutEffect(() => {
    if (!conversationId || messages.length === 0 || lastRestoredConversationId.current === conversationId) return;
    lastRestoredConversationId.current = conversationId;
    let savedPos: string | null = null;
    let savedCount: string | null = null;
    try {
      savedPos = localStorage.getItem(`${SCROLL_KEY_PREFIX}${conversationId}`);
      savedCount = localStorage.getItem(`${MSGCOUNT_KEY_PREFIX}${conversationId}`);
    } catch {
      return;
    }
    if (savedPos !== null) {
      const pos = parseInt(savedPos, 10);
      if (Number.isNaN(pos)) return;
      const parsedCount = savedCount ? parseInt(savedCount, 10) : messages.length;
      const prevCount = Number.isNaN(parsedCount) ? messages.length : parsedCount;
      const el = mainRef.current;
      if (el) {
        el.scrollTop = pos;
        lastScrollTop.current = pos;
        isPinnedToBottom.current = checkIfPinnedToBottom();
        if (messages.length > prevCount && !isPinnedToBottom.current) {
          setJumpToNewestState((prev) => (
            prev.conversationId === conversationId && prev.visible
              ? prev
              : { conversationId, visible: true }
          ));
        }
      }
    }
  }, [conversationId, messages.length, checkIfPinnedToBottom]);



  const isEmpty = messages.length === 0 && pendingMessages.length === 0;

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
              spacerHeight={spacerHeight}
              topSentinelRef={topSentinelRef}
              onRetry={onRetry}
              onCancelSteering={onCancelSteering}
              onOpenFile={onOpenFile}
            />
          )}
          {/* Streaming text — cleared atomically when sse_message arrives (REQ-CONV-019).
              Lives OUTSIDE <MessageListBody> so token updates only re-render this element,
              not the historical message list. */}
          <StreamingMessage buffer={streamingBuffer ?? null} />
        </div>
      </section>
      {showJumpToNewest && (
        <button
          className="jump-to-newest"
          onClick={() => {
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
