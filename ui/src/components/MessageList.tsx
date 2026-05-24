import { memo, useState, useEffect, useLayoutEffect, useRef, useCallback, useMemo } from 'react';
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
import {
  useBottomAnchoredWindow,
  type SavedScrollAnchor,
} from '../hooks/useBottomAnchoredWindow';
import { useUnitHeightCache } from '../conversation/unitHeightCache';
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
  /** Backend conversation UUID (atom.conversationId). Used as the
   *  localStorage namespace for the anchor and as the parent prop tying
   *  several pieces of internal state to a single conversation. */
  conversationId?: string | undefined;
  /** URL slug — the key the conversation store is keyed by. Needed by
   *  the streaming-buffer subscription in `<StreamingMessage>`. May be
   *  undefined briefly during route transitions; when undefined, the
   *  streaming tail unit is not emitted. */
  slug?: string | undefined;
  /** True while the conversation has an active streaming buffer. Stable
   *  across token mutations (the boolean stays true through every
   *  token), so this prop does not propagate per-token re-renders to
   *  `<MessageList>`. The parent derives this via `useStreamingActive`. */
  isStreaming?: boolean;
}

// Threshold in pixels - if user is within this distance of bottom, consider them "pinned"
const SCROLL_THRESHOLD = 100;

const ANCHOR_KEY_PREFIX = 'phoenix:msglist:anchor:';
// Legacy keys from the pre-render-unit scroll-restore model. We delete
// these whenever we write a fresh anchor so they don't accumulate, but
// we do not read them — one visit's worth of restore-to-bottom is the
// acceptable regression per design.md.
const LEGACY_SCROLL_KEY_PREFIX = 'phoenix:scroll:';
const LEGACY_MSGCOUNT_KEY_PREFIX = 'phoenix:msgcount:';

function parseSavedAnchor(raw: string | null): SavedScrollAnchor | null {
  if (!raw) return null;
  try {
    const parsed: unknown = JSON.parse(raw);
    if (parsed === null || typeof parsed !== 'object') return null;
    const obj = parsed as Record<string, unknown>;
    if (typeof obj['topVisibleUnitKey'] !== 'string') return null;
    if (typeof obj['offsetWithinUnit'] !== 'number') return null;
    return {
      topVisibleUnitKey: obj['topVisibleUnitKey'],
      offsetWithinUnit: obj['offsetWithinUnit'],
    };
  } catch {
    return null;
  }
}

function captureAnchor(
  scrollTop: number,
  historicalUnits: HistoricalUnit[],
  firstRenderedUnitIndex: number,
  getElement: UnitHeightObserver['getElement'],
): SavedScrollAnchor | null {
  // First rendered unit whose top is at or below the viewport top:
  // anchor + offset-into-unit covers all positions including overflow
  // past the last unit.
  for (let i = firstRenderedUnitIndex; i < historicalUnits.length; i++) {
    const unit = historicalUnits[i]!;
    const el = getElement(unit.key);
    if (el && el.offsetTop >= scrollTop) {
      return {
        topVisibleUnitKey: unit.key,
        offsetWithinUnit: scrollTop - el.offsetTop,
      };
    }
  }
  // User has scrolled past the last unit's top — anchor to the last
  // rendered unit; offsetWithinUnit absorbs the overflow.
  for (let i = historicalUnits.length - 1; i >= firstRenderedUnitIndex; i--) {
    const unit = historicalUnits[i]!;
    const el = getElement(unit.key);
    if (el) {
      return {
        topVisibleUnitKey: unit.key,
        offsetWithinUnit: scrollTop - el.offsetTop,
      };
    }
  }
  return null;
}

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
  slug: string | undefined,
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
  spacerHeight: number;
  topSentinelRef: (node: HTMLDivElement | null) => void;
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
 * ResizeObserver-attaching ref callback. The wrapper is a flex item of
 * the same parent (#messages) and visually transparent — it doesn't
 * change the layout previously produced by direct-child message
 * components, but it provides the structural DOM hook that both the
 * height cache and the saved-scroll anchor write rely on.
 */
const MessageListBody = memo(function MessageListBody({
  historicalUnits,
  tailUnits,
  firstRenderedUnitIndex,
  spacerHeight,
  topSentinelRef,
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
        .slice(firstRenderedUnitIndex)
        .map((unit) => (
          <div
            key={unit.key}
            ref={observeUnit(unit)}
            data-render-unit-key={unit.key}
          >
            {renderHistoricalUnit(unit, onOpenFile)}
          </div>
        ))}
      {tailUnits.map((unit) => renderTailUnit(unit, slug, onRetry, onCancelSteering))}
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
  isStreaming = false,
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

  // Saved unit-anchor for REQ-CONV-013, read synchronously and memoized
  // per conversation. The hook uses it to widen the initial window so the
  // anchored unit is rendered before first paint; the layout effect below
  // applies the actual scrollTop placement once the DOM is committed.
  const savedAnchorLookupRef = useRef<{ id: string | undefined; anchor: SavedScrollAnchor | null }>({
    id: undefined,
    anchor: null,
  });
  if (savedAnchorLookupRef.current.id !== conversationId) {
    let anchor: SavedScrollAnchor | null = null;
    if (conversationId) {
      try {
        anchor = parseSavedAnchor(localStorage.getItem(`${ANCHOR_KEY_PREFIX}${conversationId}`));
      } catch {
        anchor = null;
      }
    }
    savedAnchorLookupRef.current = { id: conversationId, anchor };
  }
  const savedAnchor = savedAnchorLookupRef.current.anchor;

  // streamingHandle is a tag-only TailUnit driver. The key changes only
  // on streaming-start / streaming-end transitions; it's reference-
  // stable across token mutations because `isStreaming` (boolean) is
  // stable across tokens. This keeps `historicalUnits`/`tailUnits`
  // reference-stable across tokens too — and the actual buffer text is
  // subscribed inside <StreamingMessage> via useStreamingBuffer.
  const streamingHandle = useMemo(
    () => (isStreaming && slug ? { key: `streaming-${slug}` } : null),
    [isStreaming, slug],
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
    spacerHeight,
    topSentinelRef,
  } = useBottomAnchoredWindow({
    historicalUnits,
    conversationId,
    scrollRootRef: mainRef,
    savedAnchor,
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

  // Handle scroll events to track if user is pinned to bottom and to
  // capture the current unit anchor so a later save persists the
  // up-to-date position. Fires for both user scrolls and programmatic
  // scrolls (ResizeObserver auto-anchor, restore, jump-to-newest).
  const handleScroll = useCallback(() => {
    isPinnedToBottom.current = checkIfPinnedToBottom();
    const el = mainRef.current;
    if (el) {
      lastScrollTop.current = el.scrollTop;
      if (conversationId) {
        const { historicalUnits, firstRenderedUnitIndex } = captureStateRef.current;
        const anchor = captureAnchor(
          el.scrollTop,
          historicalUnits,
          firstRenderedUnitIndex,
          unitObserver.getElement,
        );
        if (anchor) {
          latestAnchorRef.current = { conversationId, anchor };
        }
      }
    }
    if (isPinnedToBottom.current) {
      setJumpToNewestState((prev) => (
        prev.conversationId === conversationId && !prev.visible
          ? prev
          : { conversationId, visible: false }
      ));
    }
  }, [checkIfPinnedToBottom, conversationId, unitObserver]);

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

  // Latest captured anchor, refreshed on every scroll event (user
  // scrolls and programmatic scrolls both fire `scroll`, including the
  // ResizeObserver's auto-anchor-to-bottom). The save effect persists
  // the ref's value at visibilitychange / cleanup time; reading from
  // a ref is necessary because by the time the cleanup fires on
  // conversation switch, the OLD unit DOM nodes have already
  // unmounted and `unitObserver.getElement` returns nothing for them.
  const latestAnchorRef = useRef<{ conversationId: string; anchor: SavedScrollAnchor } | null>(null);

  // Latest historicalUnits + firstRenderedUnitIndex captured for the
  // scroll handler. Refs keep handleScroll's identity stable across
  // unit-content updates.
  const captureStateRef = useRef({ historicalUnits, firstRenderedUnitIndex });
  captureStateRef.current = { historicalUnits, firstRenderedUnitIndex };

  // Save unit anchor on visibility-hidden and unmount (REQ-MLRU-009 +
  // REQ-CONV-013). Replaces the prior scrollTop+messageCount save.
  useEffect(() => {
    if (!conversationId) return;
    const save = () => {
      const last = latestAnchorRef.current;
      if (last && last.conversationId === conversationId) {
        try {
          localStorage.setItem(
            `${ANCHOR_KEY_PREFIX}${conversationId}`,
            JSON.stringify(last.anchor),
          );
        } catch {
          // Quota exceeded; the in-memory state is still correct.
        }
      }
      // Prune legacy keys whether or not the new write succeeded so a
      // future visit doesn't read both shapes.
      try {
        localStorage.removeItem(`${LEGACY_SCROLL_KEY_PREFIX}${conversationId}`);
        localStorage.removeItem(`${LEGACY_MSGCOUNT_KEY_PREFIX}${conversationId}`);
      } catch {
        // ignored
      }
      // Persist any measured heights so a remount's first paint uses
      // exact spacer geometry.
      heightCache.flush();
    };
    const onVisChange = () => {
      if (document.visibilityState === 'hidden') save();
    };
    document.addEventListener('visibilitychange', onVisChange);
    return () => {
      document.removeEventListener('visibilitychange', onVisChange);
      save();
    };
  }, [conversationId, heightCache]);

  // Restore by unit anchor on first paint per conversation. useLayoutEffect
  // runs after DOM commit (unit elements registered with the observer) and
  // before the ResizeObserver fires its initial auto-scroll, so
  // isPinnedToBottom is set from the post-restore position rather than
  // racing the bottom-pin pathway.
  useLayoutEffect(() => {
    if (!conversationId) return;
    if (lastRestoredConversationId.current === conversationId) return;
    if (historicalUnits.length === 0) return;
    lastRestoredConversationId.current = conversationId;
    if (!savedAnchor) return;
    const el = unitObserver.getElement(savedAnchor.topVisibleUnitKey);
    if (!el) return; // Anchor's unit missing or not yet committed; fall back to bottom-pin.
    const root = mainRef.current;
    if (!root) return;
    root.scrollTop = el.offsetTop + savedAnchor.offsetWithinUnit;
    lastScrollTop.current = root.scrollTop;
    isPinnedToBottom.current = checkIfPinnedToBottom();
    // Seed latestAnchorRef so a save before the user's first scroll
    // event still persists the restored position.
    latestAnchorRef.current = { conversationId, anchor: savedAnchor };
  }, [conversationId, historicalUnits, savedAnchor, unitObserver, checkIfPinnedToBottom]);



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
 *   - isStreaming is a boolean that stays true through every token
 *   - callbacks are useCallback'd at the parent
 *   - slug / conversationId are URL/atom-derived strings
 *
 * The streaming buffer itself is consumed by <StreamingMessage> via
 * useStreamingBuffer(slug); only that leaf re-renders per token.
 */
export const MessageList = memo(MessageListImpl);
