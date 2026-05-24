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
  /** Backend conversation UUID (atom.conversationId). Used as the
   *  localStorage namespace for the anchor and as the parent prop tying
   *  several pieces of internal state to a single conversation. */
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
    // typeof NaN === 'number' so the typeof check alone admits NaN
    // (localStorage is user-mutable). Number.isFinite tightens to real
    // finite numbers so a corrupted offsetWithinUnit can't coerce
    // scrollTop to NaN at restore time.
    const offset = obj['offsetWithinUnit'];
    if (typeof offset !== 'number' || !Number.isFinite(offset)) return null;
    const unitCountAtSave = obj['unitCountAtSave'];
    const validCount = typeof unitCountAtSave === 'number'
      && Number.isFinite(unitCountAtSave)
      && unitCountAtSave >= 0
      ? unitCountAtSave
      : undefined;
    return {
      topVisibleUnitKey: obj['topVisibleUnitKey'],
      offsetWithinUnit: offset,
      ...(validCount !== undefined ? { unitCountAtSave: validCount } : {}),
    };
  } catch {
    return null;
  }
}

/** Distance from the scroll root's content top to an element's top —
 *  the correct generalization of `el.offsetTop` when the scroll root
 *  is not the offsetParent. Uses `getBoundingClientRect` (viewport
 *  coordinates) then converts to content coordinates by adding
 *  `root.scrollTop` and subtracting the root's own viewport offset. */
function unitTopInScrollRoot(el: HTMLElement, root: HTMLElement): number {
  return el.getBoundingClientRect().top
    - root.getBoundingClientRect().top
    + root.scrollTop;
}

function captureAnchor(
  scrollTop: number,
  historicalUnits: HistoricalUnit[],
  firstRenderedUnitIndex: number,
  root: HTMLElement,
  getElement: UnitHeightObserver['getElement'],
): SavedScrollAnchor | null {
  // Walk rendered units in DOM order; the LAST unit whose top is at or
  // above scrollTop is the unit the viewport-top intersects (the unit
  // the user is reading). offsetWithinUnit = positive distance from
  // the unit's top to the viewport top.
  //
  // If no rendered unit's top is <= scrollTop (user is above all
  // rendered units, i.e. inside the spacer), fall through to the
  // fallback below.
  let visibleTopUnit: { unit: HistoricalUnit; unitTop: number } | null = null;
  for (let i = firstRenderedUnitIndex; i < historicalUnits.length; i++) {
    const unit = historicalUnits[i]!;
    const el = getElement(unit.key);
    if (!el) continue;
    const unitTop = unitTopInScrollRoot(el, root);
    if (unitTop <= scrollTop) {
      visibleTopUnit = { unit, unitTop };
    } else {
      // Once we pass scrollTop, subsequent units are also below — done.
      break;
    }
  }
  if (visibleTopUnit) {
    return {
      topVisibleUnitKey: visibleTopUnit.unit.key,
      offsetWithinUnit: scrollTop - visibleTopUnit.unitTop,
      unitCountAtSave: historicalUnits.length,
    };
  }
  // Fallback: user is above all rendered units (inside the spacer) —
  // anchor to the first rendered unit. offsetWithinUnit is negative
  // here, restore lands at unitTop + negativeOffset = the same spacer
  // position. Restore math handles either sign.
  for (let i = firstRenderedUnitIndex; i < historicalUnits.length; i++) {
    const unit = historicalUnits[i]!;
    const el = getElement(unit.key);
    if (el) {
      return {
        topVisibleUnitKey: unit.key,
        offsetWithinUnit: scrollTop - unitTopInScrollRoot(el, root),
        unitCountAtSave: historicalUnits.length,
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
          el,
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

  // Restore by unit anchor on first paint per conversation. Runs in
  // useLayoutEffect so it lands before the ResizeObserver's first
  // observation has a chance to interfere.
  //
  // Three things have to land together here (REQ-MLRU-009, REQ-CONV-013):
  //   1. scrollTop placement at the saved anchor (when anchor + unit
  //      both exist), using unitTopInScrollRoot so the math is
  //      correct regardless of whether #main-area is the offsetParent.
  //   2. prevMessagesHeight seeded from the current content height so
  //      the first ResizeObserver tick (which compares against this
  //      ref) does not see a bogus heightGrew=true and either snap to
  //      bottom (clobbering the restore) or pop the "↓ New messages"
  //      button on a fresh visit.
  //   3. latestAnchorRef seeded so a quick visibility-hidden after
  //      restore (user opens conversation and tab-switches) persists
  //      the restored position rather than nothing — and so the saved
  //      unitCountAtSave is preserved across save/restore cycles.
  //
  // The "new messages while away" surface is the saved
  // unitCountAtSave vs current historicalUnits.length comparison: if
  // the count grew and the restored position is not at bottom, show
  // the jump-to-newest button.
  useLayoutEffect(() => {
    if (!conversationId) return;
    if (lastRestoredConversationId.current === conversationId) return;
    if (historicalUnits.length === 0) return;
    lastRestoredConversationId.current = conversationId;

    const root = mainRef.current;
    const messagesEl = messagesRef.current;
    if (!root || !messagesEl) return;

    // (2) Seed prevMessagesHeight so the first ResizeObserver tick
    //     after this restore doesn't observe a spurious "height grew".
    prevMessagesHeight.current = messagesEl.getBoundingClientRect().height;

    if (savedAnchor) {
      const el = unitObserver.getElement(savedAnchor.topVisibleUnitKey);
      if (el) {
        // (1) Restore with correct content-coordinate math.
        const unitTop = unitTopInScrollRoot(el, root);
        root.scrollTop = unitTop + savedAnchor.offsetWithinUnit;
        lastScrollTop.current = root.scrollTop;
        isPinnedToBottom.current = checkIfPinnedToBottom();
      }
    }

    // (3) Seed latestAnchorRef with the current position (or, if the
    //     anchor is missing, with whatever bottom-pin we landed on)
    //     so a save before the first scroll persists meaningful state.
    const initialAnchor = captureAnchor(
      root.scrollTop,
      historicalUnits,
      firstRenderedUnitIndex,
      root,
      unitObserver.getElement,
    );
    if (initialAnchor) {
      latestAnchorRef.current = { conversationId, anchor: initialAnchor };
    }

    // jumpToNewest surface for "new messages arrived while away":
    // the saved anchor's unitCountAtSave (if present) tells us how
    // many units existed at save time. If the count grew AND we're
    // not currently pinned to bottom, surface the button.
    if (
      savedAnchor?.unitCountAtSave !== undefined
      && historicalUnits.length > savedAnchor.unitCountAtSave
      && !isPinnedToBottom.current
    ) {
      setJumpToNewestState((prev) => (
        prev.conversationId === conversationId && prev.visible
          ? prev
          : { conversationId, visible: true }
      ));
    }
  }, [
    conversationId,
    historicalUnits,
    savedAnchor,
    firstRenderedUnitIndex,
    unitObserver,
    checkIfPinnedToBottom,
  ]);



  // isEmpty reflects "nothing for MessageListBody to render". Includes
  // tail units (pending_user, sub_agent_status, streaming_agent) so the
  // empty-state placeholder doesn't hide an active streaming buffer or
  // sub-agent status block when historical messages are still empty
  // (e.g., sub-agent flow that begins streaming before the user message
  // persists, or a reconnect race where pending events outpace the
  // initial snapshot).
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
