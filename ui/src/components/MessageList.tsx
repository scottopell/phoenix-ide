import { useEffect, useRef, useCallback } from 'react';
import type { Message, ToolResultContent, ConversationState } from '../api';
import type { QueuedMessage } from '../hooks';
import {
  UserMessage,
  QueuedUserMessage,
  AgentMessage,
  SubAgentStatus,
} from './MessageComponents';

interface MessageListProps {
  messages: Message[];
  queuedMessages: QueuedMessage[];
  convState: string;
  stateData: ConversationState | null;
  onRetry: (localId: string) => void;
  onOpenFile: ((filePath: string, modifiedLines: Set<number>, firstModifiedLine: number) => void) | undefined;
}

// Threshold in pixels - if user is within this distance of bottom, consider them "pinned"
const SCROLL_THRESHOLD = 100;

export function MessageList({ messages, queuedMessages, convState, stateData, onRetry, onOpenFile }: MessageListProps) {
  const mainRef = useRef<HTMLElement>(null);
  const isPinnedToBottom = useRef(true); // Start pinned to bottom
  const prevMessagesLength = useRef(messages.length);

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
  }, [checkIfPinnedToBottom]);

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
                // Skip tool messages - they're rendered inline with their tool_use
                return null;
              })}
              {/* Render queued messages (sending state) */}
              {sendingMessages.map((msg) => (
                <QueuedUserMessage key={msg.localId} message={msg} onRetry={onRetry} />
              ))}
              {convState === 'awaiting_sub_agents' && stateData && (
                <SubAgentStatus stateData={stateData} />
              )}
            </>
          )}
        </div>
      </section>
    </main>
  );
}
