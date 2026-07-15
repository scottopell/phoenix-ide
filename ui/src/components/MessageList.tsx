import {
  memo,
  useState,
  useRef,
  useCallback,
  useMemo,
  useEffect,
  useLayoutEffect,
  forwardRef,
  useImperativeHandle,
} from 'react';
import { useDensity } from '../hooks/useDensity';
import {
  FindBar,
  buildConversationSearchProjection,
  useViewerFindKeyboardShortcut,
} from './viewer-find';
import { useFocusScope, useFocusScopeCommands } from '../hooks/useFocusScope';
import {
  VirtualTranscript,
  type VirtualTranscriptHandle,
  type VirtualTranscriptPhysicalSnapshot,
  type VirtualTranscriptRange,
  type VirtualTranscriptRangeChange,
} from './VirtualTranscript';
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
import { useStreamingBuffer, useStreamingRequestId } from '../conversation/useConversationAtom';
import {
  buildHistoricalUnits,
  findHistoricalUnitIndexByMessageId,
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
import type { HistoryView, RestoreBasis } from '../conversation/historyExpansion';
import {
  initialTranscriptPositioningState,
  reduceTranscriptPositioning,
  type TranscriptPositioningEffect,
  type TranscriptPositioningEvent,
  type TranscriptPositioningInput,
} from '../conversation/transcriptPositioning';

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

const RESTORE_OFFSET_TOLERANCE_PX = 2;

function historyViewKey(view: HistoryView): string {
  return `${view.conversationId}:${view.generation}:${view.transcriptGeneration}`;
}

function scheduleDeferred(callback: () => void): () => void {
  let cancelled = false;
  queueMicrotask(() => {
    if (!cancelled) callback();
  });
  return () => {
    cancelled = true;
  };
}

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
  onOpenCommissionReview?: ((requestSequenceId: number) => void) | undefined;
  systemPrompt?: string | undefined;
  conversationId?: string | undefined;
  slug?: string | undefined;
  filePathRootDir?: string | undefined;
  workScopeKey?: string | undefined;
  enableMessageSidepanel?: boolean | undefined;
  /** Scroll-spy: the inclusive range of `historicalUnits`/virtual transcript item
   *  indices currently rendered. Fired as the user scrolls. The conversation nav
   *  uses it to highlight the active chapter. */
  onVisibleRangeChange?: ((range: VirtualTranscriptRange) => void) | undefined;
  /** Conversation chapters derived from the SAME `historicalUnits` array this
   *  list feeds to the virtual transcript. MessageList owns the build, so the
   *  chapter `unitIndex` values are guaranteed to be in virtual transcript
   *  coordinate space — no second `buildRenderUnits` pass to drift against. */
  onChaptersChange?: ((chapters: Chapter[]) => void) | undefined;
  hasOlderMessages?: boolean | undefined;
  onLoadOlderMessages?: ((restoreBasis?: RestoreBasis) => void) | undefined;
  loadingOlderMessages?: boolean | undefined;
  olderHistoryError?: string | null | undefined;
  transcriptPositioning: TranscriptPositioningInput;
  onHistoryScrollCommandHandled?: ((token: number, result: 'applied' | 'target_missing' | 'superseded', view: HistoryView) => void) | undefined;
}

/** Imperative surface exposed to the conversation nav strip. MessageList owns
 *  virtual transcript ref; the nav can't reach it directly because off-screen
 *  rows are unmounted, so a querySelector jump would miss them. */
export interface MessageListHandle {
  /** Scroll the render unit at `unitIndex` (a `historicalUnits` index, which
   *  equals its virtual transcript item index) into view and pulse it once mounted. */
  scrollToUnitIndex: (unitIndex: number) => void;
  scrollToMessageId: (messageId: string) => boolean;
  captureHistoryRestoreBasis: () => RestoreBasis;
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
type OnOpenCommissionReview = ((requestSequenceId: number) => void) | undefined;

function activeToolUseIdFromState(convState: ConversationState): string | undefined {
  if (convState.type !== 'tool_executing' && convState.type !== 'cancelling_tool') return undefined;
  const id = convState.current_tool?.id;
  return typeof id === 'string' && id.length > 0 ? id : undefined;
}

function renderHistoricalUnit(
  unit: HistoricalUnit,
  onOpenFile: OnOpenFile,
  onOpenCommissionReview: OnOpenCommissionReview,
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
        <div id={`message-${unit.message.message_id}`} className="message user" data-sequence-id={unit.message.sequence_id}>
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
          onOpenCommissionReview={onOpenCommissionReview}
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
  onOpenCommissionReview: OnOpenCommissionReview,
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
  return renderHistoricalUnit(unit, onOpenFile, onOpenCommissionReview, filePathRootDir, onRetry, onCancelSteering, workScopeKey, activeToolUseId, isLatestAgentMessage);
}

interface SystemPromptHeaderProps {
  systemPrompt: string;
  expanded: boolean;
  onToggle: () => void;
  contentRef: React.RefObject<HTMLPreElement>;
}

const SystemPromptHeader = memo(function SystemPromptHeader({
  systemPrompt,
  expanded,
  onToggle,
  contentRef,
}: SystemPromptHeaderProps) {
  return (
    <div className="virtual-transcript-row">
      <div className={`system-prompt-block${expanded ? ' expanded' : ''}`}>
        <div className="system-prompt-header" onClick={onToggle}>
          <span className="system-prompt-label">System prompt</span>
          <span className="system-prompt-toggle">
            {expanded ? <ChevronDown /> : <ChevronRight />}
            {expanded ? ' hide' : ' show'}
          </span>
        </div>
        {expanded && <pre ref={contentRef} className="system-prompt-content">{systemPrompt}</pre>}
      </div>
    </div>
  );
});

function EmptyTranscriptState() {
  return (
    <div className="empty-state">
      <div className="empty-state-icon"><MessageSquareIcon /></div>
      <p>Start a conversation</p>
    </div>
  );
}

function OpenFindStreamingBuffer({ slug, onChange }: { slug: string; onChange: (buffer: import('../conversation/atom').StreamingBuffer | null) => void }) {
  const buffer = useStreamingBuffer(slug);
  useEffect(() => onChange(buffer), [buffer, onChange]);
  return null;
}

function MessageListImpl({
  messages,
  pendingMessages,
  convState,
  onRetry,
  onCancelSteering,
  onOpenFile,
  onOpenCommissionReview,
  systemPrompt,
  conversationId,
  slug,
  filePathRootDir,
  workScopeKey,
  enableMessageSidepanel = true,
  onVisibleRangeChange,
  onChaptersChange,
  hasOlderMessages = false,
  onLoadOlderMessages,
  loadingOlderMessages = false,
  olderHistoryError,
  transcriptPositioning,
  onHistoryScrollCommandHandled,
}: MessageListProps, ref: React.ForwardedRef<MessageListHandle>) {
  const findScopeId = `conversation-transcript:${conversationId ?? 'empty'}`;
  const { activeScope } = useFocusScope();
  const { pushScope, popScope } = useFocusScopeCommands();
  const { density } = useDensity();
  const [findOpen, setFindOpen] = useState(false);
  const [findFocusVersion, setFindFocusVersion] = useState(0);
  const [findQuery, setFindQuery] = useState('');
  const [findActiveIndex, setFindActiveIndex] = useState(0);
  const [findStreamingBuffer, setFindStreamingBuffer] = useState<import('../conversation/atom').StreamingBuffer | null>(null);
  const findPreviousFocusRef = useRef<HTMLElement | null>(null);
  const [systemPromptExpanded, setSystemPromptExpanded] = useState(false);
  const systemPromptRef = useRef<HTMLPreElement | null>(null);
  const [hasUnreadTailContent, setHasUnreadTailContent] = useState(false);
  const firstVisibleUnitIndexRef = useRef(0);
  const continuityRestoreInFlightRef = useRef(false);
  const transcriptRef = useRef<VirtualTranscriptHandle>(null);
  const scrollerRef = useRef<HTMLElement | null>(null);
  const scrollMachineRef = useRef(initialScrollMachineState(conversationId));

  // The streaming buffer's `requestId` IS the eventual agent message_id
  // (server uses the same uuid for both — see `AssistantMessage::new` in
  // crates/phoenix-ide/src/state_machine/state.rs). Keying the streaming
  // unit by this value means the finalized `agent_turn` HistoricalUnit
  // arrives under the same render-unit key, and the transition is an
  // in-place keyed update — the virtual transcript doesn't observe a key swap, the
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
    () => buildTailUnits({
      convState,
      streamingHandle,
      endsInAgentRun,
      finalizedAgentKeys: new Set(
        historicalUnits
          .filter((unit) => unit.kind === 'agent_turn')
          .map((unit) => unit.key),
      ),
    }),
    [convState, streamingHandle, endsInAgentRun, historicalUnits],
  );

  const allUnits = useMemo<RenderUnit[]>(
    () => [...historicalUnits, ...tailUnits],
    [historicalUnits, tailUnits],
  );

  const latestAgentKey = useMemo(() => {
    for (let i = historicalUnits.length - 1; i >= 0; i -= 1) {
      const unit = historicalUnits[i];
      if (unit?.kind === 'agent_turn') return unit.key;
    }
    return null;
  }, [historicalUnits]);

  const findProjection = useMemo(
    () => (findOpen && findQuery.length > 0
      ? buildConversationSearchProjection(allUnits, findQuery, {
          density,
          latestAgentKey,
          streamingBuffer: findStreamingBuffer,
          systemPrompt: systemPrompt ?? null,
          systemPromptExpanded,
        })
      : { sources: [], matches: [] }),
    [allUnits, density, findOpen, findQuery, findStreamingBuffer, latestAgentKey, systemPrompt, systemPromptExpanded],
  );
  const findMatches = findProjection.matches;
  const normalizedFindIndex = findMatches.length === 0 ? -1 : Math.min(findActiveIndex, findMatches.length - 1);
  const activeFindMatch = findOpen && normalizedFindIndex >= 0 ? findMatches[normalizedFindIndex] ?? null : null;
  const activeFindMatchRef = useRef(activeFindMatch);
  activeFindMatchRef.current = activeFindMatch;
  const activeFindMatchKey = activeFindMatch
    ? `${activeFindMatch.target.kind}:${activeFindMatch.target.sourceId}:${activeFindMatch.start}:${activeFindMatch.end}`
    : null;
  const findRowByKey = useCallback((key: string): Element | null => {
    const scroller = scrollerRef.current;
    if (!scroller) return null;
    try {
      return scroller.querySelector(`[data-render-unit-key="${CSS.escape(key)}"]`);
    } catch {
      return null;
    }
  }, []);
  const openFind = useCallback(() => {
    findPreviousFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    setFindOpen(true);
    setFindFocusVersion((version) => version + 1);
  }, []);
  const closeFind = useCallback(() => {
    setFindOpen(false);
    requestAnimationFrame(() => findPreviousFocusRef.current?.focus());
  }, []);

  useEffect(() => {
    if (!findOpen) return undefined;
    pushScope(findScopeId);
    return () => popScope(findScopeId);
  }, [findOpen, findScopeId, popScope, pushScope]);
  useViewerFindKeyboardShortcut({
    scopeId: findScopeId,
    onOpen: openFind,
    allowWhenNoActiveScope: true,
  });
  useEffect(() => {
    if (!findOpen) return undefined;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      if (activeScope !== findScopeId) return;
      const target = event.target as HTMLElement | null;
      const isBodyFindInput = target instanceof HTMLElement && target.dataset['viewerFindInput'] === 'true';
      if (isBodyFindInput) return;
      event.preventDefault();
      event.stopPropagation();
      closeFind();
    };
    window.addEventListener('keydown', handleKeyDown, true);
    return () => window.removeEventListener('keydown', handleKeyDown, true);
  }, [activeScope, closeFind, findOpen, findScopeId]);
  const findConversationRef = useRef(conversationId);
  useEffect(() => {
    if (findConversationRef.current === conversationId) return;
    findConversationRef.current = conversationId;
    if (findOpen) setFindOpen(false);
    if (findQuery) setFindQuery('');
    if (findActiveIndex !== 0) setFindActiveIndex(0);
  }, [conversationId, findActiveIndex, findOpen, findQuery]);
  const changeFindQuery = useCallback((query: string) => {
    setFindQuery(query);
    setFindActiveIndex(0);
  }, []);
  const nextFindMatch = useCallback(() => {
    setFindActiveIndex(() => {
      if (findMatches.length === 0) return 0;
      return (normalizedFindIndex + 1) % findMatches.length;
    });
  }, [findMatches.length, normalizedFindIndex]);
  const previousFindMatch = useCallback(() => {
    setFindActiveIndex(() => {
      if (findMatches.length === 0) return 0;
      return (normalizedFindIndex - 1 + findMatches.length) % findMatches.length;
    });
  }, [findMatches.length, normalizedFindIndex]);

  // Chapters are derived here, not in a parent, so they share the exact
  // `historicalUnits` array the virtual transcript renders — a chapter's
  // `unitIndex` is therefore a valid `scrollToIndex` target with no second build to drift
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

  const detachGestureListenersRef = useRef<(() => void) | null>(null);
  const settleSnapRafRef = useRef(0);
  const tailFollowRafRef = useRef(0);
  const settleWatchTimerRef = useRef(0);
  const dispatchScrollEventRef = useRef<(event: ScrollEvent) => void>(() => {});
  const transcriptPositioningStateRef = useRef(initialTranscriptPositioningState(
    transcriptPositioning.kind === 'idle' ? transcriptPositioning.view : transcriptPositioning.command.view,
  ));
  const dispatchTranscriptPositioningRef = useRef<(event: TranscriptPositioningEvent) => void>(() => {});
  const lastPhysicalSnapshotRef = useRef<VirtualTranscriptPhysicalSnapshot | null>(null);
  const transcriptPositioningViewKeyRef = useRef(historyViewKey(
    transcriptPositioning.kind === 'idle' ? transcriptPositioning.view : transcriptPositioning.command.view,
  ));
  const executorAttachEpochRef = useRef(0);
  const cancelPendingExecutorDetachRef = useRef<(() => void) | null>(null);
  const earlierHistoryRequestScheduledRef = useRef(false);
  const requestEarlierHistoryRef = useRef<(retry: boolean) => void>(() => {});

  const readScrollSnapshot = useCallback((): ScrollSnapshot | null => {
    const s = scrollerRef.current;
    return s ? { scrollHeight: s.scrollHeight, scrollTop: s.scrollTop, clientHeight: s.clientHeight } : null;
  }, []);

  const handlePinnedStateChange = useCallback((pinned: boolean) => {
    const snapshot = readScrollSnapshot();
    const domAtBottom = snapshot
      ? snapshot.scrollHeight - snapshot.scrollTop - snapshot.clientHeight <= PIN_TO_BOTTOM_THRESHOLD
      : pinned;
    dispatchScrollEventRef.current({ type: 'viewportPinnedChanged', atBottom: pinned || domAtBottom });
  }, [readScrollSnapshot]);

  const scheduleDomBottomWrite = useCallback(() => {
    if (settleSnapRafRef.current !== 0) return;
    settleSnapRafRef.current = requestAnimationFrame(() => {
      settleSnapRafRef.current = 0;
      dispatchScrollEventRef.current({ type: 'settleProbe', snapshot: readScrollSnapshot(), nowMs: Date.now() });
    });
  }, [readScrollSnapshot]);

  const scheduleTailFollow = useCallback((conversationIdAtSchedule: string | undefined) => {
    if (tailFollowRafRef.current !== 0) {
      cancelAnimationFrame(tailFollowRafRef.current);
    }
    tailFollowRafRef.current = requestAnimationFrame(() => {
      tailFollowRafRef.current = 0;
      const machine = scrollMachineRef.current;
      const authorized =
        machine.kind === 'live' &&
        machine.conversationId === conversationIdAtSchedule &&
        machine.follow.kind !== 'reading' &&
        machine.follow.kind !== 'navigating' &&
        !(machine.gesture.kind === 'touch' && machine.gesture.moved);
      if (authorized) {
        transcriptRef.current?.scrollToTail();
      }
    });
  }, []);

  const stopSettleWatch = useCallback(() => {
    if (settleSnapRafRef.current !== 0) {
      cancelAnimationFrame(settleSnapRafRef.current);
      settleSnapRafRef.current = 0;
    }
    if (tailFollowRafRef.current !== 0) {
      cancelAnimationFrame(tailFollowRafRef.current);
      tailFollowRafRef.current = 0;
    }
    if (settleWatchTimerRef.current !== 0) {
      clearInterval(settleWatchTimerRef.current);
      settleWatchTimerRef.current = 0;
    }
  }, []);

  const startSettleWatch = useCallback(() => {
    scheduleDomBottomWrite();
    if (settleWatchTimerRef.current !== 0) return;
    settleWatchTimerRef.current = window.setInterval(() => {
      dispatchScrollEventRef.current({ type: 'settleProbe', snapshot: readScrollSnapshot(), nowMs: Date.now() });
    }, SETTLE_WATCH_INTERVAL_MS);
  }, [readScrollSnapshot, scheduleDomBottomWrite]);

  const applyScrollEffects = useCallback((effects: ScrollEffect[]) => {
    for (const effect of effects) {
      switch (effect.type) {
        case 'snapToLastIndex':
          transcriptRef.current?.scrollToTail();
          break;
        case 'scheduleTailFollow':
          scheduleTailFollow(effect.conversationId);
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
      }
    }
  }, [scheduleDomBottomWrite, scheduleTailFollow, startSettleWatch, stopSettleWatch]);

  const dispatchScrollEvent = useCallback((event: ScrollEvent) => {
    const next = reduceScrollMachine(scrollMachineRef.current, event);
    scrollMachineRef.current = next.state;
    applyScrollEffects(next.effects);
  }, [applyScrollEffects]);
  dispatchScrollEventRef.current = dispatchScrollEvent;

  useEffect(() => {
    return stopSettleWatch;
  }, [stopSettleWatch]);

  const handleScrollerRef = useCallback((ref: HTMLElement | Window | null) => {
    detachGestureListenersRef.current?.();
    detachGestureListenersRef.current = null;
    scrollerRef.current = ref instanceof HTMLElement ? ref : null;
    if (ref instanceof HTMLElement) {
      ref.id = 'messages';
      ref.dataset['appScrollOwner'] = '';
      dispatchScrollEvent({
        type: 'scrollerAttached',
        snapshot: { scrollHeight: ref.scrollHeight, scrollTop: ref.scrollTop, clientHeight: ref.clientHeight },
      });
      const onPointerDown = () => {
        dispatchTranscriptPositioningRef.current({ type: 'user_interrupted' });
        dispatchScrollEvent({ type: 'interactionStarted' });
      };
      const onTouchStart = () => {
        dispatchTranscriptPositioningRef.current({ type: 'user_interrupted' });
        dispatchScrollEvent({ type: 'touchStarted' });
      };
      const onTouchMove = () => dispatchScrollEvent({ type: 'touchMoved' });
      const onTouchEnd = (e: TouchEvent) => dispatchScrollEvent({ type: 'touchEnded', remainingTouches: e.touches.length });
      const onTouchCancel = (e: TouchEvent) => dispatchScrollEvent({ type: 'touchCancelled', remainingTouches: e.touches.length });
      const onWheel = (e: WheelEvent) => {
        dispatchTranscriptPositioningRef.current({ type: 'user_interrupted' });
        dispatchScrollEvent({ type: 'interactionStarted' });
        if (e.deltaY < 0) {
          dispatchScrollEvent({ type: 'upwardIntent' });
          const visibleRange = transcriptRef.current?.physicalSnapshot().visibleRange;
          if (visibleRange && visibleRange.startIndex <= 2) requestEarlierHistoryRef.current(false);
        }
      };
      const onScroll = () => {
        if (continuityRestoreInFlightRef.current) {
          const active = transcriptPositioningStateRef.current.active;
          const phase = transcriptPositioningStateRef.current.phase;
          if (active?.command.kind === 'restore_after_prefix_expansion' && phase?.kind === 'awaiting_physical') {
            const snapshot = transcriptRef.current?.physicalSnapshot(phase.targetIndex) ?? null;
            const actualOffset = snapshot?.targetOffset ?? null;
            if (actualOffset !== null && Math.abs(actualOffset - active.command.viewportStartOffset) > RESTORE_OFFSET_TOLERANCE_PX) {
              dispatchTranscriptPositioningRef.current({ type: 'user_interrupted' });
            }
          }
          return;
        }
        const snapshot = { scrollHeight: ref.scrollHeight, scrollTop: ref.scrollTop, clientHeight: ref.clientHeight };
        const machine = scrollMachineRef.current;
        const previousTop = machine.kind === 'live' || machine.kind === 'mount-rescue'
          ? machine.geometry.lastSnapshot?.scrollTop ?? snapshot.scrollTop
          : snapshot.scrollTop;
        dispatchScrollEvent(
          snapshot.scrollTop < previousTop
            ? { type: 'upwardIntent', snapshot }
            : { type: 'downwardMovement', snapshot },
        );
      };
      ref.addEventListener('pointerdown', onPointerDown, { passive: true });
      ref.addEventListener('touchstart', onTouchStart, { passive: true });
      ref.addEventListener('touchmove', onTouchMove, { passive: true });
      ref.addEventListener('touchend', onTouchEnd, { passive: true });
      ref.addEventListener('touchcancel', onTouchCancel, { passive: true });
      ref.addEventListener('wheel', onWheel, { passive: true });
      ref.addEventListener('scroll', onScroll, { passive: true });
      detachGestureListenersRef.current = () => {
        ref.removeEventListener('pointerdown', onPointerDown);
        ref.removeEventListener('touchstart', onTouchStart);
        ref.removeEventListener('touchmove', onTouchMove);
        ref.removeEventListener('touchend', onTouchEnd);
        ref.removeEventListener('touchcancel', onTouchCancel);
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

  const prevMessagesLengthRef = useRef(messages.length);
  const prevStreamingRequestIdRef = useRef(streamingRequestId);
  const prevPendingLengthRef = useRef(pendingMessages.length);
  const prevConversationIdRef = useRef(conversationId);
  const streamingRequestIdRef = useRef(streamingRequestId);
  const convStateRef = useRef(convState);
  streamingRequestIdRef.current = streamingRequestId;
  convStateRef.current = convState;

  useEffect(() => {
    const conversationChanged = prevConversationIdRef.current !== conversationId;
    const messagesGrew = messages.length > prevMessagesLengthRef.current;
    const streamingStarted =
      prevStreamingRequestIdRef.current === null && streamingRequestId !== null;
    const pendingGrew = pendingMessages.length > prevPendingLengthRef.current;

    prevConversationIdRef.current = conversationId;
    prevMessagesLengthRef.current = messages.length;
    prevStreamingRequestIdRef.current = streamingRequestId;
    prevPendingLengthRef.current = pendingMessages.length;

    if (conversationChanged) {
      lastPhysicalSnapshotRef.current = null;
      dispatchScrollEvent({ type: 'conversationChanged', conversationId });
      return;
    }
    if (messagesGrew || streamingStarted || pendingGrew) {
      dispatchScrollEvent({ type: 'tailContentAdvanced' });
    }
  }, [conversationId, dispatchScrollEvent, messages.length, pendingMessages.length, streamingRequestId]);

  const handleTotalListHeightChanged = useCallback((newHeight: number) => {
    const tailActivity: TailActivity =
      streamingRequestIdRef.current !== null || convStateRef.current.type === 'awaiting_sub_agents'
        ? 'active'
        : 'none';
    const machine = scrollMachineRef.current;
    const measurement = {
      totalHeight: newHeight,
      unitCount: allUnitsLengthRef.current,
      snapshot: readScrollSnapshot(),
    };
    dispatchScrollEvent(
      (machine.kind === 'unmeasured' || machine.kind === 'measured-empty') ||
      machine.conversationId !== conversationId
      ? {
          type: 'conversationMeasured',
          conversationId,
          ...measurement,
          nowMs: Date.now(),
        }
      : {
          type: 'heightChanged',
          ...measurement,
          tailActivity,
        });
  }, [conversationId, dispatchScrollEvent, readScrollSnapshot]);

  const scrollToNewest = useCallback(() => {
    dispatchScrollEvent({ type: 'jumpToNewestRequested', unitCount: allUnitsLengthRef.current });
  }, [dispatchScrollEvent]);

  // A navigation jump has one positioning owner: VirtualTranscript. The selected
  // key remains pending until its virtualized row mounts, at which point the row
  // ref applies presentation-only highlighting without moving the scroller.
  const pendingPulseRef = useRef<{ conversationId: string | undefined; key: string } | null>(null);
  const highlightedTargetRef = useRef<Element | null>(null);
  const pulseTimerRef = useRef(0);

  const clearHighlight = useCallback(() => {
    if (pulseTimerRef.current !== 0) {
      clearTimeout(pulseTimerRef.current);
      pulseTimerRef.current = 0;
    }
    highlightedTargetRef.current?.classList.remove('jump-highlight');
    highlightedTargetRef.current = null;
  }, []);

  const pulseMountedRow = useCallback((key: string, row: HTMLDivElement | null) => {
    const pending = pendingPulseRef.current;
    if (
      row === null ||
      pending === null ||
      pending.conversationId !== conversationId ||
      pending.key !== key
    ) return;
    pendingPulseRef.current = null;
    clearHighlight();
    const target = row.querySelector('.message') ?? row;
    highlightedTargetRef.current = target;
    target.classList.add('jump-highlight');
    pulseTimerRef.current = window.setTimeout(() => {
      target.classList.remove('jump-highlight');
      if (highlightedTargetRef.current === target) highlightedTargetRef.current = null;
      pulseTimerRef.current = 0;
    }, 1500);
  }, [clearHighlight, conversationId]);

  useEffect(() => () => {
    pendingPulseRef.current = null;
    clearHighlight();
  }, [clearHighlight]);

  useEffect(() => {
    pendingPulseRef.current = null;
    clearHighlight();
  }, [conversationId, clearHighlight]);


  const pulseIfMounted = useCallback((key: string) => {
    const row = Array.from(scrollerRef.current?.querySelectorAll<HTMLDivElement>('[data-render-unit-key]') ?? [])
      .find((candidate) => candidate.dataset['renderUnitKey'] === key);
    if (row) pulseMountedRow(key, row);
  }, [pulseMountedRow]);

  const scrollToUnitIndex = useCallback((unitIndex: number) => {
    const unit = historicalUnits[unitIndex];
    if (!unit) return;
    dispatchScrollEvent({ type: 'navigationJumped' });
    clearHighlight();
    pendingPulseRef.current = { conversationId, key: unit.key };
    transcriptRef.current?.scrollToIndex(unitIndex, 'start');
    pulseIfMounted(unit.key);
  }, [historicalUnits, clearHighlight, conversationId, dispatchScrollEvent, pulseIfMounted]);

  const findUnitIndexByMessageId = useCallback(
    (messageId: string) => findHistoricalUnitIndexByMessageId(historicalUnits, messageId),
    [historicalUnits],
  );

  const messageIdForHistoricalUnit = useCallback((unit: HistoricalUnit): string | null => {
    if (unit.kind === 'agent_turn') return unit.agent.message_id;
    if ('message' in unit && 'message_id' in unit.message) return unit.message.message_id;
    return null;
  }, []);

  const scrollToMessageId = useCallback((messageId: string) => {
    const index = findUnitIndexByMessageId(messageId);
    if (index < 0) return false;
    scrollToUnitIndex(index);
    return true;
  }, [findUnitIndexByMessageId, scrollToUnitIndex]);

  const captureHistoryRestoreBasis = useCallback((): RestoreBasis => {
    const machine = scrollMachineRef.current;
    if (machine.kind === 'mount-rescue' || (machine.kind === 'live' && machine.follow.kind !== 'reading')) {
      return { kind: 'following_tail' };
    }
    const anchor = transcriptRef.current?.captureVisibleAnchor();
    if (!anchor) return { kind: 'following_tail' };
    const unit = historicalUnits[anchor.index];
    if (!unit || unit.key !== anchor.key) return { kind: 'following_tail' };
    const messageId = messageIdForHistoricalUnit(unit);
    if (!messageId) return { kind: 'following_tail' };
    return { kind: 'reader_anchor', messageId, viewportStartOffset: anchor.offset };
  }, [historicalUnits, messageIdForHistoricalUnit]);

  const requestEarlierHistory = useCallback((retry: boolean) => {
    const machine = scrollMachineRef.current;
    const readerOwnsViewport = machine.kind === 'live'
      && machine.conversationId === conversationId
      && machine.follow.kind === 'reading';
    if (
      earlierHistoryRequestScheduledRef.current
      || !hasOlderMessages
      || loadingOlderMessages
      || !onLoadOlderMessages
      || (!retry && (olderHistoryError || !readerOwnsViewport))
    ) return;
    earlierHistoryRequestScheduledRef.current = true;
    const restoreBasis = captureHistoryRestoreBasis();
    queueMicrotask(() => onLoadOlderMessages(restoreBasis));
  }, [captureHistoryRestoreBasis, conversationId, hasOlderMessages, loadingOlderMessages, olderHistoryError, onLoadOlderMessages]);
  requestEarlierHistoryRef.current = requestEarlierHistory;

  useEffect(() => {
    earlierHistoryRequestScheduledRef.current = false;
  }, [conversationId]);

  useEffect(() => {
    if (!hasOlderMessages || olderHistoryError) earlierHistoryRequestScheduledRef.current = false;
  }, [hasOlderMessages, olderHistoryError]);

  useImperativeHandle(
    ref,
    () => ({ scrollToUnitIndex, scrollToMessageId, captureHistoryRestoreBasis }),
    [captureHistoryRestoreBasis, scrollToMessageId, scrollToUnitIndex],
  );

  const applyTranscriptPositioningEffects = useCallback((effects: TranscriptPositioningEffect[]) => {
    for (const effect of effects) {
      switch (effect.type) {
        case 'resolve_target': {
          const targetIndex = findUnitIndexByMessageId(effect.targetMessageId);
          dispatchTranscriptPositioningRef.current(
            targetIndex < 0
              ? { type: 'target_missing', commandKey: effect.commandKey }
              : { type: 'target_resolved', commandKey: effect.commandKey, targetIndex },
          );
          break;
        }
        case 'position': {
          const command = effect.command;
          if (command.kind === 'jump_to_message') {
            const unit = historicalUnits[effect.targetIndex];
            if (unit) {
              dispatchScrollEvent({ type: 'navigationJumped' });
              clearHighlight();
              pendingPulseRef.current = { conversationId, key: unit.key };
              if (effect.viewportStartOffset === undefined) {
                transcriptRef.current?.scrollToIndex(effect.targetIndex, effect.align);
              } else {
                transcriptRef.current?.scrollToIndex(effect.targetIndex, effect.align, effect.viewportStartOffset);
              }
              pulseIfMounted(unit.key);
            }
          } else {
            continuityRestoreInFlightRef.current = true;
            transcriptRef.current?.scrollToIndex(effect.targetIndex, effect.align, effect.viewportStartOffset);
          }
          const physicalSnapshot = transcriptRef.current?.physicalSnapshot(effect.targetIndex)
            ?? lastPhysicalSnapshotRef.current
            ?? { renderedRange: null, visibleRange: null, viewportTop: 0, layoutRevision: 0, targetIndex: effect.targetIndex, targetOffset: null };
          lastPhysicalSnapshotRef.current = physicalSnapshot;
          dispatchTranscriptPositioningRef.current({
            type: 'position_issued',
            commandKey: effect.commandKey,
            targetIndex: effect.targetIndex,
            layoutRevision: physicalSnapshot.layoutRevision,
          });
          dispatchTranscriptPositioningRef.current({
            type: 'physical_observed',
            commandKey: effect.commandKey,
            range: physicalSnapshot.visibleRange,
            actualOffset: command.kind === 'restore_after_prefix_expansion'
              ? physicalSnapshot.targetOffset ?? null
              : null,
            layoutRevision: physicalSnapshot.layoutRevision,
            targetMeasured: physicalSnapshot.targetMeasured ?? false,
          });
          break;
        }
        case 'finish':
          if (effect.command.kind === 'restore_after_prefix_expansion') {
            continuityRestoreInFlightRef.current = false;
          }
          onHistoryScrollCommandHandled?.(effect.command.token, effect.result, effect.command.view);
          break;
      }
    }
  }, [clearHighlight, conversationId, dispatchScrollEvent, findUnitIndexByMessageId, historicalUnits, onHistoryScrollCommandHandled, pulseIfMounted]);

  const dispatchTranscriptPositioning = useCallback((event: TranscriptPositioningEvent) => {
    const next = reduceTranscriptPositioning(transcriptPositioningStateRef.current, event);
    transcriptPositioningStateRef.current = next.state;
    applyTranscriptPositioningEffects(next.effects);
  }, [applyTranscriptPositioningEffects]);
  dispatchTranscriptPositioningRef.current = dispatchTranscriptPositioning;

  useLayoutEffect(() => {
    const attachEpoch = executorAttachEpochRef.current + 1;
    executorAttachEpochRef.current = attachEpoch;
    cancelPendingExecutorDetachRef.current?.();
    cancelPendingExecutorDetachRef.current = null;
    return () => {
      const cancel = scheduleDeferred(() => {
        if (executorAttachEpochRef.current !== attachEpoch) return;
        cancelPendingExecutorDetachRef.current = null;
        dispatchTranscriptPositioningRef.current({ type: 'executor_detached' });
      });
      cancelPendingExecutorDetachRef.current = cancel;
    };
  }, []);

  useLayoutEffect(() => {
    const nextView = transcriptPositioning.kind === 'idle' ? transcriptPositioning.view : transcriptPositioning.command.view;
    const nextViewKey = historyViewKey(nextView);
    if (transcriptPositioningViewKeyRef.current !== nextViewKey) {
      lastPhysicalSnapshotRef.current = null;
      transcriptPositioningViewKeyRef.current = nextViewKey;
    }
    dispatchTranscriptPositioning({ type: 'input_changed', input: transcriptPositioning });
  }, [dispatchTranscriptPositioning, transcriptPositioning]);

  useEffect(() => {
    scrollerRef.current?.querySelectorAll('.viewer-find-row-match, .viewer-find-row-match--active')
      .forEach((element) => element.classList.remove('viewer-find-row-match', 'viewer-find-row-match--active'));
    const match = activeFindMatchRef.current;
    if (!match) return undefined;
    dispatchScrollEvent({ type: 'navigationJumped' });
    if (match.target.kind === 'header-text') {
      const timers = [0, 80, 220].map((delay) => window.setTimeout(() => {
        const header = systemPromptRef.current;
        if (!header) return;
        scrollerRef.current?.querySelectorAll('.viewer-find-row-match, .viewer-find-row-match--active')
          .forEach((element) => element.classList.remove('viewer-find-row-match', 'viewer-find-row-match--active'));
        header.classList.add('viewer-find-row-match', 'viewer-find-row-match--active');
        header.scrollIntoView({ block: 'center' });
      }, delay));
      return () => timers.forEach(clearTimeout);
    }
    const unitMatch = match.target;
    transcriptRef.current?.scrollToIndex(unitMatch.unitIndex, 'start');
    const timers = [80, 220, 500].map((delay) => window.setTimeout(() => {
      const row = findRowByKey(unitMatch.unitKey);
      if (!row) return;
      scrollerRef.current?.querySelectorAll('.viewer-find-row-match, .viewer-find-row-match--active')
        .forEach((element) => element.classList.remove('viewer-find-row-match', 'viewer-find-row-match--active'));
      row.classList.add('viewer-find-row-match', 'viewer-find-row-match--active');
      row.scrollIntoView({ block: 'center' });
    }, delay));
    return () => timers.forEach(clearTimeout);
  }, [activeFindMatchKey, dispatchScrollEvent, findRowByKey]);

  useEffect(() => {
    if (findOpen || activeScope !== findScopeId) return;
    scrollerRef.current?.querySelectorAll('.viewer-find-row-match, .viewer-find-row-match--active')
      .forEach((element) => element.classList.remove('viewer-find-row-match', 'viewer-find-row-match--active'));
  }, [activeScope, findOpen, findScopeId]);

  const handleRangeChanged = useCallback((snapshot: VirtualTranscriptRangeChange) => {
    const visibleRange = snapshot.visibleRange;
    const renderedRange = snapshot.renderedRange;
    lastPhysicalSnapshotRef.current = snapshot;
    if (!visibleRange && !renderedRange) return;
    firstVisibleUnitIndexRef.current = visibleRange?.startIndex ?? renderedRange!.startIndex;
    const active = transcriptPositioningStateRef.current.active;
    const phase = transcriptPositioningStateRef.current.phase;
    if (active && phase?.kind === 'awaiting_physical') {
      const physicalSnapshot = transcriptRef.current?.physicalSnapshot(phase.targetIndex) ?? snapshot;
      dispatchTranscriptPositioningRef.current({
        type: 'physical_observed',
        commandKey: active.key,
        range: physicalSnapshot.visibleRange,
        actualOffset: active.command.kind === 'restore_after_prefix_expansion'
          ? physicalSnapshot.targetOffset ?? null
          : null,
        layoutRevision: physicalSnapshot.layoutRevision,
        targetMeasured: physicalSnapshot.targetMeasured ?? false,
      });
    }
    if (visibleRange) {
      onVisibleRangeChange?.(visibleRange);
      if (visibleRange.startIndex <= 2) requestEarlierHistory(false);
    }
  }, [onVisibleRangeChange, requestEarlierHistory]);

  const toggleSystemPrompt = useCallback(() => {
    setSystemPromptExpanded((v) => !v);
  }, []);


  const itemContent = useCallback(
    (unit: RenderUnit) => (
      <div
        className="virtual-transcript-row"
        data-render-unit-key={unit.key}
        ref={(row) => pulseMountedRow(unit.key, row)}
      >
        {renderUnit(unit, slug, onOpenFile, onOpenCommissionReview, filePathRootDir, onRetry, onCancelSteering, workScopeKey, activeToolUseId, unit.kind === 'agent_turn' && unit.key === latestAgentKey)}
      </div>
    ),
    [slug, onOpenFile, onOpenCommissionReview, filePathRootDir, onRetry, onCancelSteering, workScopeKey, activeToolUseId, latestAgentKey, pulseMountedRow],
  );

  const computeItemKey = useCallback(
    (unit: RenderUnit) => unit.key,
    [],
  );

  return (
    <main id="main-area" className="chat-main-area">
      {findOpen && (
        <>
          {slug && <OpenFindStreamingBuffer slug={slug} onChange={setFindStreamingBuffer} />}
          <FindBar
            query={findQuery}
            activeIndex={normalizedFindIndex}
            matchCount={findMatches.length}
            focusVersion={findFocusVersion}
            onQueryChange={changeFindQuery}
            onNext={nextFindMatch}
            onPrevious={previousFindMatch}
            onClose={closeFind}
          />
        </>
      )}
      <section id="chat-view" className="view active">
        {olderHistoryError && hasOlderMessages && onLoadOlderMessages && (
          <div role="alert">
            <span>Could not load earlier history.</span>
            <button type="button" onClick={() => requestEarlierHistory(true)}>
              Retry
            </button>
          </div>
        )}
        {olderHistoryError && !hasOlderMessages && (
          <div role="alert">Could not load earlier history: {olderHistoryError}</div>
        )}
        <VirtualTranscript
          key={conversationId ?? '__empty__'}
          ref={transcriptRef}
          scrollerId="messages"
          scrollerRef={handleScrollerRef}
          items={allUnits}
          renderItem={itemContent}
          getKey={computeItemKey}
          initialTail={allUnits.length > 0}
          estimatedExtent={120}
          overscan={600}
          onPinnedChange={handlePinnedStateChange}
          onTotalExtentChange={handleTotalListHeightChanged}
          onRangeChange={handleRangeChanged}
          header={systemPrompt ? (
            <SystemPromptHeader
              systemPrompt={systemPrompt}
              expanded={systemPromptExpanded}
              onToggle={toggleSystemPrompt}
              contentRef={systemPromptRef}
            />
          ) : null}
          empty={<EmptyTranscriptState />}
          className="message-virtual-transcript"
        />
      </section>
      {!isEmpty && hasUnreadTailContent && (
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
