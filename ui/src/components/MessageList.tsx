import { useEffect, useRef } from 'react';
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
}

export function MessageList({ messages, queuedMessages, convState, stateData, onRetry }: MessageListProps) {
  const mainRef = useRef<HTMLElement>(null);

  // Scroll to bottom when messages change
  useEffect(() => {
    if (mainRef.current) {
      mainRef.current.scrollTop = mainRef.current.scrollHeight;
    }
  }, [messages, queuedMessages, convState]);

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
    <main id="main-area" ref={mainRef}>
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
