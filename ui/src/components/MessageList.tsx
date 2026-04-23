import { memo, useState, useEffect, useRef, useCallback, useMemo } from 'react';
import type { Message, ToolResultContent, ConversationState } from '../api';
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
  queuedMessages: QueuedMessage[];
  convState: ConversationState;
  onRetry: (localId: string) => void;
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

interface MessageListBodyProps {
  messages: Message[];
  sendingMessages: QueuedMessage[];
  toolResults: Map<string, Message>;
  convState: ConversationState;
  onRetry: (localId: string) => void;
  onOpenFile: ((filePath: string, modifiedLines: Set<number>, firstModifiedLine: number) => void) | undefined;
}

/**
 * Memoized subtree holding the .map over historical messages. Wrapping this
 * in React.memo means token updates to `streamingBuffer` (which flow through
 * the parent MessageList shell) DO NOT cause the historical messages to
 * re-render — shallow prop compare skips the subtree entirely because
 * `messages`, `toolResults`, etc. are reference-stable across token arrivals.
 */
const MessageListBody = memo(function MessageListBody({
  messages,
  sendingMessages,
  toolResults,
  convState,
  onRetry,
  onOpenFile,
}: MessageListBodyProps) {
  return (
    <>
      {messages.map((msg) => {
        const type = msg.message_type || msg.type;
        if (type === 'user') {
          return <UserMessage key={msg.sequence_id} message={msg} />;
        } else if (type === 'skill') {
          const skillContent = msg.content as { name?: string; trigger?: string };
          const skillTrigger = skillContent.trigger || '';
          const triggerArgs = extractSkillArgs(skillTrigger, skillContent.name || '');
          return (
            <div key={msg.sequence_id} className="message user" data-sequence-id={msg.sequence_id}>
              <div className="message-header">
                <span className="message-sender">You</span>
                {msg.created_at && (
                  <span className="message-time" title={new Date(msg.created_at).toLocaleString()}>
                    {formatMessageTime(msg.created_at)}
                  </span>
                )}
              </div>
              <div className="message-content">
                <div className="skill-indicator" title={`Skill invocation: loaded instructions from /${skillContent.name || 'skill'}/SKILL.md and delivered to the agent`}>
                  <span className="skill-label">skill: /{skillContent.name || 'skill'}</span>
                  {triggerArgs && (
                    <span className="skill-trigger">{triggerArgs}</span>
                  )}
                </div>
              </div>
            </div>
          );
        } else if (type === 'agent') {
          return (
            <AgentMessage
              key={msg.sequence_id}
              message={msg}
              toolResults={toolResults}
              onOpenFile={onOpenFile}
            />
          );
        }
        if (type === 'system') {
          const text = (msg.content as { text?: string })?.text;
          if (text) {
            return (
              <div key={msg.sequence_id} className="system-message">
                <span className="system-message-text">{text}</span>
              </div>
            );
          }
        }
        // Skip tool messages - they're rendered inline with their tool_use
        return null;
      })}
      {/* Render queued messages (sending state) */}
      {sendingMessages.map((msg) => (
        <QueuedUserMessage key={msg.localId} message={msg} onRetry={onRetry} />
      ))}
      {convState.type === 'awaiting_sub_agents' && (
        <SubAgentStatus stateData={convState} />
      )}
    </>
  );
});

export function MessageList({
  messages,
  queuedMessages,
  convState,
  onRetry,
  onOpenFile,
  systemPrompt,
  conversationId,
  streamingBuffer,
}: MessageListProps) {
  const [systemPromptExpanded, setSystemPromptExpanded] = useState(false);
  const [showJumpToNewest, setShowJumpToNewest] = useState(false);
  const mainRef = useRef<HTMLElement>(null);
  const messagesRef = useRef<HTMLDivElement>(null);
  const isPinnedToBottom = useRef(true); // Start pinned to bottom
  const scrollRestored = useRef(false);
  const initialMessageCount = useRef<number | null>(null);
  const lastScrollTop = useRef(0);
  const prevMessagesHeight = useRef(0);

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
    if (showJumpToNewest && isPinnedToBottom.current) {
      setShowJumpToNewest(false);
    }
  }, [checkIfPinnedToBottom, showJumpToNewest]);

  // Scroll to bottom helper
  const scrollToBottom = useCallback(() => {
    if (mainRef.current) {
      mainRef.current.scrollTop = mainRef.current.scrollHeight;
    }
  }, []);

  // Single ResizeObserver drives all auto-scroll.
  // Fires after layout is complete (unlike rAF which fires before paint), so
  // scrollHeight is always the settled value — no mid-render jumps.
  // Triggers on any content growth: streaming tokens, new messages, new tool blocks.
  // Does NOT trigger on phase-only state changes that produce no visible content.
  useEffect(() => {
    const messagesEl = messagesRef.current;
    if (!messagesEl) return;

    const observer = new ResizeObserver((entries) => {
      for (const entry of entries) {
        const newHeight = entry.contentRect.height;
        if (newHeight > prevMessagesHeight.current) {
          if (isPinnedToBottom.current) {
            // Content grew and user is pinned — follow it
            mainRef.current!.scrollTop = mainRef.current!.scrollHeight;
          } else {
            // Content grew but user scrolled up — nudge them
            setShowJumpToNewest(true);
          }
        }
        prevMessagesHeight.current = newHeight;
      }
    });

    observer.observe(messagesEl);
    return () => observer.disconnect();
  }, []);

  // Save scroll position on unmount / visibility change (REQ-UI-013)
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

  // Restore scroll position on mount after messages render (REQ-UI-013)
  useEffect(() => {
    if (!conversationId || messages.length === 0 || scrollRestored.current) return;
    scrollRestored.current = true;
    const savedPos = localStorage.getItem(`${SCROLL_KEY_PREFIX}${conversationId}`);
    const savedCount = localStorage.getItem(`${MSGCOUNT_KEY_PREFIX}${conversationId}`);
    if (savedPos !== null) {
      const pos = parseInt(savedPos, 10);
      const prevCount = savedCount ? parseInt(savedCount, 10) : messages.length;
      requestAnimationFrame(() => {
        const el = mainRef.current;
        if (!el) return;
        el.scrollTop = pos;
        lastScrollTop.current = pos;
        isPinnedToBottom.current = checkIfPinnedToBottom();
        // Show "jump to newest" if new messages arrived while away
        if (messages.length > prevCount && !isPinnedToBottom.current) {
          setShowJumpToNewest(true);
        }
      });
    }
    // Record initial message count for jump-to-newest logic
    initialMessageCount.current = messages.length;
  }, [conversationId, messages.length, checkIfPinnedToBottom]);

  // Reset scrollRestored when conversation changes
  useEffect(() => {
    scrollRestored.current = false;
    prevMessagesHeight.current = 0;
    isPinnedToBottom.current = true; // Re-pin on every conversation switch
    setShowJumpToNewest(false);
    initialMessageCount.current = null;
  }, [conversationId]);



  // Build a map of tool_use_id -> tool result for pairing
  const toolResults = useMemo(() => {
    const map = new Map<string, Message>();
    for (const msg of messages) {
      const type = msg.message_type || msg.type;
      if (type === 'tool') {
        const content = msg.content as ToolResultContent;
        const toolUseId = content?.tool_use_id;
        if (toolUseId) {
          map.set(toolUseId, msg);
        }
      }
    }
    return map;
  }, [messages]);

  // Get queued messages that are in "sending" state (not failed - those show in InputArea)
  const sendingMessages = useMemo(
    () => queuedMessages.filter(m => m.status === 'sending'),
    [queuedMessages],
  );

  const isEmpty = messages.length === 0 && sendingMessages.length === 0;

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
              messages={messages}
              sendingMessages={sendingMessages}
              toolResults={toolResults}
              convState={convState}
              onRetry={onRetry}
              onOpenFile={onOpenFile}
            />
          )}
          {/* Streaming text — cleared atomically when sse_message arrives (REQ-UI-019).
              Lives OUTSIDE <MessageListBody> so token updates only re-render this element,
              not the historical message list. */}
          <StreamingMessage buffer={streamingBuffer ?? null} />
        </div>
      </section>
      {showJumpToNewest && (
        <button
          className="jump-to-newest"
          onClick={() => { scrollToBottom(); setShowJumpToNewest(false); }}
        >
          ↓ New messages
        </button>
      )}
      <MessageContextMenu messages={messages} />
    </main>
  );
}
