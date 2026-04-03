import { useState, useEffect, useRef, useCallback } from 'react';
import type { Message, ToolResultContent, ConversationState } from '../api'; // ConversationState used in MessageListProps
import type { QueuedMessage } from '../hooks';
import type { StreamingBuffer } from '../conversation/atom';
import {
  UserMessage,
  QueuedUserMessage,
  AgentMessage,
  SubAgentStatus,
} from './MessageComponents';
import { StreamingMessage } from './StreamingMessage';

interface MessageListProps {
  messages: Message[];
  queuedMessages: QueuedMessage[];
  convState: ConversationState;
  onRetry: (localId: string) => void;
  onOpenFile: ((filePath: string, modifiedLines: Set<number>, firstModifiedLine: number) => void) | undefined;
  systemPrompt?: string;
  conversationId?: string | undefined;
  streamingBuffer?: StreamingBuffer | null;
}

// Threshold in pixels - if user is within this distance of bottom, consider them "pinned"
const SCROLL_THRESHOLD = 100;

const SCROLL_KEY_PREFIX = 'phoenix:scroll:';
const MSGCOUNT_KEY_PREFIX = 'phoenix:msgcount:';

export function MessageList({ messages, queuedMessages, convState, onRetry, onOpenFile, systemPrompt, conversationId, streamingBuffer }: MessageListProps) {
  const [systemPromptExpanded, setSystemPromptExpanded] = useState(false);
  const [showJumpToNewest, setShowJumpToNewest] = useState(false);
  const mainRef = useRef<HTMLElement>(null);
  const isPinnedToBottom = useRef(true); // Start pinned to bottom
  const prevMessagesLength = useRef(messages.length);
  const scrollRestored = useRef(false);
  const initialMessageCount = useRef<number | null>(null);
  const lastScrollTop = useRef(0);

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

  // Auto-scroll only when pinned to bottom and content changes
  useEffect(() => {
    // Always scroll to bottom when new messages are added (user sent or received new message)
    const messagesAdded = messages.length > prevMessagesLength.current;
    prevMessagesLength.current = messages.length;

    if (messagesAdded && isPinnedToBottom.current) {
      // Use requestAnimationFrame to ensure DOM has updated
      requestAnimationFrame(() => {
        scrollToBottom();
      });
    }
  }, [messages.length, scrollToBottom]);

  // Also scroll when queued messages change (user is sending)
  useEffect(() => {
    if (isPinnedToBottom.current && queuedMessages.length > 0) {
      requestAnimationFrame(() => {
        scrollToBottom();
      });
    }
  }, [queuedMessages.length, scrollToBottom]);

  // Scroll on state changes only if pinned
  useEffect(() => {
    if (isPinnedToBottom.current) {
      requestAnimationFrame(() => {
        scrollToBottom();
      });
    }
  }, [convState, scrollToBottom]);

  // Auto-scroll when streaming buffer grows (REQ-UI-019)
  useEffect(() => {
    if (!streamingBuffer) return;
    if (isPinnedToBottom.current) {
      requestAnimationFrame(() => {
        scrollToBottom();
      });
    } else {
      // User has scrolled up — show "jump to live" affordance
      setShowJumpToNewest(true);
    }
  }, [streamingBuffer?.text, scrollToBottom]); // eslint-disable-line react-hooks/exhaustive-deps

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
    setShowJumpToNewest(false);
    initialMessageCount.current = null;
  }, [conversationId]);



  // Build a map of tool_use_id -> tool result for pairing
  const toolResults = new Map<string, Message>();
  for (const msg of messages) {
    const type = msg.message_type || msg.type;
    if (type === 'tool') {
      const content = msg.content as ToolResultContent;
      const toolUseId = content?.tool_use_id;
      if (toolUseId) {
        toolResults.set(toolUseId, msg);
      }
    }
  }

  // Get queued messages that are in "sending" state (not failed - those show in InputArea)
  const sendingMessages = queuedMessages.filter(m => m.status === 'sending');

  return (
    <main id="main-area" ref={mainRef} onScroll={handleScroll}>
      <section id="chat-view" className="view active">
        <div id="messages">
          {systemPrompt && (
            <div className={`system-prompt-block${systemPromptExpanded ? ' expanded' : ''}`}>
              <div
                className="system-prompt-header"
                onClick={() => setSystemPromptExpanded((v) => !v)}
              >
                <span className="system-prompt-label">System prompt</span>
                <span className="system-prompt-toggle">
                  {systemPromptExpanded ? '▼ hide' : '▶ show'}
                </span>
              </div>
              {systemPromptExpanded && (
                <pre className="system-prompt-content">{systemPrompt}</pre>
              )}
            </div>
          )}
          {messages.length === 0 && sendingMessages.length === 0 ? (
            <div className="empty-state">
              <div className="empty-state-icon">✨</div>
              <p>Start a conversation</p>
            </div>
          ) : (
            <>
              {messages.map((msg) => {
                const type = msg.message_type || msg.type;
                if (type === 'user') {
                  return <UserMessage key={msg.sequence_id} message={msg} />;
                } else if (type === 'skill') {
                  const skillContent = msg.content as { name?: string; trigger?: string };
                  const skillTrigger = skillContent.trigger || '';
                  const triggerArgs = skillTrigger.replace(new RegExp(`^/?${skillContent.name || ''}\\s*`), '').trim();
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
              {/* Streaming text — cleared atomically when sse_message arrives (REQ-UI-019) */}
              <StreamingMessage buffer={streamingBuffer ?? null} />
            </>
          )}
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
    </main>
  );
}
