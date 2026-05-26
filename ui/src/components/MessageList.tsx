import { memo, useState, useRef, useCallback, useMemo } from 'react';
import { Virtuoso, type VirtuosoHandle } from 'react-virtuoso';
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
import { useStreamingRequestId } from '../conversation/useConversationAtom';
import {
  buildRenderUnits,
  type HistoricalUnit,
  type TailUnit,
  type RenderUnit,
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
  pendingMessages: QueuedMessage[];
  convState: ConversationState;
  onRetry: (localId: string) => void;
  onCancelSteering?: ((localId: string) => void) | undefined;
  onOpenFile: ((filePath: string, modifiedLines: Set<number>, firstModifiedLine: number) => void) | undefined;
  systemPrompt?: string | undefined;
  conversationId?: string | undefined;
  slug?: string | undefined;
}

const PIN_TO_BOTTOM_THRESHOLD = 100;
// Wider tolerance for the re-snap-on-content-growth path than for the
// pin-detection threshold above. The streaming agent's item grows
// asynchronously as markdown / code blocks mount; that growth pushes
// the user beyond the 100px pin threshold without any user input. Treat
// "within 300px of bottom" as "drifted by render lag, snap back."
const FOLLOW_OUTPUT_DRIFT_TOLERANCE = 300;

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
      return <StreamingMessage slug={slug} />;
  }
}

function renderUnit(
  unit: RenderUnit,
  slug: string | undefined,
  onOpenFile: OnOpenFile,
  onRetry: (localId: string) => void,
  onCancelSteering: ((localId: string) => void) | undefined,
): JSX.Element | null {
  if (unit.kind === 'sub_agent_status' || unit.kind === 'streaming_agent') {
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
}: MessageListProps) {
  const [systemPromptExpanded, setSystemPromptExpanded] = useState(false);
  const [isAtBottom, setIsAtBottom] = useState(true);
  const virtuosoRef = useRef<VirtuosoHandle>(null);
  const scrollerRef = useRef<HTMLElement | null>(null);

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

  const isEmpty = allUnits.length === 0;

  const handleAtBottomStateChange = useCallback((atBottom: boolean) => {
    setIsAtBottom(atBottom);
  }, []);

  const handleScrollerRef = useCallback((ref: HTMLElement | Window | null) => {
    scrollerRef.current = ref instanceof HTMLElement ? ref : null;
  }, []);

  // virtuoso's `followOutput="auto"` only fires when `data.length` grows;
  // it doesn't re-snap when the LAST item's height changes async after
  // mount (markdown render, react-syntax-highlighter mounting, image
  // loading). That leaves the user a few hundred pixels above true bottom
  // — visually "not at the bottom" despite virtuoso's internal pin.
  //
  // Bridge the gap: when virtuoso reports the total list height changed,
  // check the live scroller's distance to bottom. If within a generous
  // drift-tolerant threshold (wider than `atBottomThreshold=100` so post-
  // mount growth doesn't kick us out), imperatively re-snap. Past the
  // threshold means the user has intentionally scrolled up — respect
  // that and let the jump-to-newest button do its job.
  const handleTotalListHeightChanged = useCallback(() => {
    const s = scrollerRef.current;
    if (!s) return;
    const fromBottom = s.scrollHeight - s.scrollTop - s.clientHeight;
    if (fromBottom < FOLLOW_OUTPUT_DRIFT_TOLERANCE) {
      virtuosoRef.current?.scrollToIndex({
        index: 'LAST',
        align: 'end',
        behavior: 'auto',
      });
    }
  }, []);

  const scrollToNewest = useCallback(() => {
    virtuosoRef.current?.scrollToIndex({
      index: 'LAST',
      align: 'end',
      behavior: 'auto',
    });
  }, []);

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

  return (
    <main id="main-area">
      <section id="chat-view" className="view active">
        {isEmpty && !systemPrompt ? (
          <div className="empty-state">
            <div className="empty-state-icon"><MessageSquareIcon /></div>
            <p>Start a conversation</p>
          </div>
        ) : (
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
            initialTopMostItemIndex={{ index: 'LAST', align: 'end' }}
            alignToBottom
            increaseViewportBy={{ top: 600, bottom: 600 }}
            {...(SystemPromptHeaderSlot ? { components: { Header: SystemPromptHeaderSlot } } : {})}
            className="message-virtuoso"
          />
        )}
      </section>
      {!isEmpty && !isAtBottom && (
        <button className="jump-to-newest" onClick={scrollToNewest}>
          ↓ New messages
        </button>
      )}
      <MessageContextMenu messages={messages} />
    </main>
  );
}

export const MessageList = memo(MessageListImpl);
