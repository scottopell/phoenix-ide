import { useEffect, useRef, useState } from 'react';
import type { Message, ContentBlock, ToolResultContent, ConversationState } from '../api';
import type { QueuedMessage } from '../hooks';
import { escapeHtml, renderMarkdown, formatRelativeTime } from '../utils';
import { CopyButton } from './CopyButton';

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

function formatMessageTime(isoStr: string): string {
  if (!isoStr) return '';
  const date = new Date(isoStr);
  const now = new Date();
  const isToday = date.toDateString() === now.toDateString();
  
  if (isToday) {
    return date.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
  }
  return date.toLocaleDateString([], { month: 'short', day: 'numeric' });
}

function UserMessage({ message }: { message: Message }) {
  const content = message.content as { text?: string; images?: { data: string; media_type: string }[] };
  const text = content.text || (typeof message.content === 'string' ? message.content : '');
  const images = content.images || [];
  const timestamp = message.created_at;

  return (
    <div className="message user">
      <div className="message-header">
        <span className="message-sender">You</span>
        {timestamp && (
          <span className="message-time" title={new Date(timestamp).toLocaleString()}>
            {formatMessageTime(timestamp)}
          </span>
        )}
        <span className="message-status sent" title="Sent">✓</span>
      </div>
      <div className="message-content">
        {escapeHtml(text)}
        {images.length > 0 && (
          <div style={{ marginTop: 8, color: 'var(--text-muted)', fontSize: 13 }}>
            [{images.length} image(s)]
          </div>
        )}
      </div>
    </div>
  );
}

function QueuedUserMessage({ message, onRetry }: { message: QueuedMessage; onRetry: (localId: string) => void }) {
  const isFailed = message.status === 'failed';
  const isSending = message.status === 'sending';

  return (
    <div className={`message user ${isFailed ? 'failed' : ''}`}>
      <div className="message-header">
        <span>You</span>
        {isSending && (
          <span className="message-status sending" title="Sending...">
            <span className="sending-spinner">⏳</span>
          </span>
        )}
        {isFailed && (
          <span 
            className="message-status failed" 
            title="Failed - tap to retry"
            onClick={() => onRetry(message.localId)}
            style={{ cursor: 'pointer' }}
          >
            ⚠️
          </span>
        )}
      </div>
      <div className="message-content">
        {escapeHtml(message.text)}
        {message.images.length > 0 && (
          <div style={{ marginTop: 8, color: 'var(--text-muted)', fontSize: 13 }}>
            [{message.images.length} image(s)]
          </div>
        )}
      </div>
    </div>
  );
}

function AgentMessage({
  message,
  toolResults,
}: {
  message: Message;
  toolResults: Map<string, Message>;
}) {
  const blocks = Array.isArray(message.content) ? (message.content as ContentBlock[]) : [];
  const timestamp = message.created_at;

  return (
    <div className="message agent">
      <div className="message-header">
        <span className="message-sender">Phoenix</span>
        {timestamp && (
          <span className="message-time" title={new Date(timestamp).toLocaleString()}>
            {formatMessageTime(timestamp)}
          </span>
        )}
      </div>
      <div className="message-content">
        {blocks.map((block, i) => {
          if (block.type === 'text') {
            return (
              <div
                key={i}
                dangerouslySetInnerHTML={{ __html: renderMarkdown(block.text || '') }}
              />
            );
          } else if (block.type === 'tool_use') {
            return (
              <ToolUseBlock
                key={block.id || i}
                block={block}
                result={toolResults.get(block.id || '')}
              />
            );
          }
          return null;
        })}
      </div>
    </div>
  );
}

// Thresholds for auto-expanding output
const OUTPUT_AUTO_EXPAND_THRESHOLD = 200;  // Always show inline if under this
const OUTPUT_PREVIEW_THRESHOLD = 500;      // Show preview if under this

function formatToolInput(name: string, input: Record<string, unknown>): { display: string; isMultiline: boolean } {
  switch (name) {
    case 'bash': {
      const cmd = String(input.command || '');
      return { display: `$ ${cmd}`, isMultiline: cmd.includes('\n') };
    }
    case 'think': {
      const thoughts = String(input.thoughts || '');
      return { display: thoughts, isMultiline: thoughts.includes('\n') };
    }
    case 'patch': {
      const path = String(input.path || '');
      const patches = input.patches as Array<{ operation?: string }> | undefined;
      const op = patches?.[0]?.operation || 'modify';
      const count = patches?.length || 1;
      const summary = count > 1 ? `${path}: ${count} patches` : `${path}: ${op}`;
      return { display: summary, isMultiline: false };
    }
    case 'keyword_search': {
      const query = String(input.query || '');
      const terms = (input.search_terms as string[]) || [];
      const termsStr = terms.length > 0 ? terms.slice(0, 3).join(', ') + (terms.length > 3 ? '...' : '') : '';
      return { display: termsStr ? `"${query}" [${termsStr}]` : query, isMultiline: false };
    }
    case 'read_image': {
      const path = String(input.path || '');
      return { display: path, isMultiline: false };
    }
    default: {
      const str = JSON.stringify(input, null, 2);
      return { display: str, isMultiline: str.includes('\n') };
    }
  }
}

function ToolUseBlock({ block, result }: { block: ContentBlock; result?: Message }) {
  const name = block.name || 'tool';
  const input = block.input || {};
  const toolId = block.id || '';

  // Format the input display based on tool type
  const { display: inputDisplay, isMultiline: inputIsMultiline } = formatToolInput(name, input as Record<string, unknown>);

  // Get the paired result if available
  let resultContent: ToolResultContent | null = null;
  if (result) {
    resultContent = result.content as ToolResultContent;
  }

  const resultText = resultContent?.content || resultContent?.result || resultContent?.error || '';
  const isError = resultContent?.is_error || !!resultContent?.error;
  const resultLength = resultText.length;

  // Determine if output should be auto-expanded
  const shouldAutoExpand = resultLength > 0 && resultLength < OUTPUT_AUTO_EXPAND_THRESHOLD;
  const [outputExpanded, setOutputExpanded] = useState(shouldAutoExpand);

  // For display, truncate very long outputs even when expanded
  const maxDisplayLen = 5000;
  const displayResult = resultText.length > maxDisplayLen 
    ? resultText.slice(0, maxDisplayLen) + `\n... (${resultText.length - maxDisplayLen} more chars)`
    : resultText;

  // Preview for collapsed state
  const previewLen = 100;
  const previewText = resultText.length > previewLen 
    ? resultText.slice(0, previewLen).split('\n')[0] + '...'
    : resultText.split('\n')[0];

  const hasOutput = resultContent !== null;
  const isShortOutput = resultLength < OUTPUT_AUTO_EXPAND_THRESHOLD;

  // Get the raw input for copying (not the formatted display)
  const rawInput = name === 'bash' ? String(input.command || '') : 
                   name === 'think' ? String(input.thoughts || '') :
                   JSON.stringify(input, null, 2);

  return (
    <div className="tool-block" data-tool-id={toolId}>
      {/* Tool header with name */}
      <div className="tool-block-header">
        <span className="tool-block-name">{name}</span>
        {hasOutput && (
          <span className={`tool-block-status ${isError ? 'error' : 'success'}`}>
            {isError ? '✗' : '✓'}
          </span>
        )}
      </div>

      {/* Tool input - always visible */}
      <div className={`tool-block-input ${inputIsMultiline ? 'multiline' : ''}`}>
        {inputDisplay}
        <CopyButton text={rawInput} title="Copy command" />
      </div>

      {/* Tool output - collapsible for long outputs */}
      {hasOutput && (
        <div className={`tool-block-output ${isError ? 'error' : ''} ${outputExpanded ? 'expanded' : ''}`}>
          {isShortOutput ? (
            // Short output: show inline, no collapse
            <div className="tool-block-output-content">
              {displayResult || <span className="tool-empty">(empty)</span>}
              {resultText && <CopyButton text={resultText} title="Copy output" />}
            </div>
          ) : (
            // Long output: collapsible
            <>
              <div 
                className="tool-block-output-header" 
                onClick={() => setOutputExpanded(!outputExpanded)}
              >
                <span className="tool-block-output-chevron">{outputExpanded ? '▼' : '▶'}</span>
                <span className="tool-block-output-label">
                  {outputExpanded ? 'output' : previewText}
                </span>
                <span className="tool-block-output-size">({resultLength.toLocaleString()} chars)</span>
                <CopyButton text={resultText} title="Copy output" />
              </div>
              {outputExpanded && (
                <div className="tool-block-output-content">
                  {displayResult}
                </div>
              )}
            </>
          )}
        </div>
      )}
    </div>
  );
}

function SubAgentStatus({ stateData }: { stateData: ConversationState }) {
  const pending = stateData.pending_ids?.length ?? 0;
  const completed = stateData.completed_results?.length ?? 0;
  const total = pending + completed;

  return (
    <div className="subagent-status-block">
      <div className="subagent-header">
        <span className="subagent-title">Sub-agents</span>
        <span className="subagent-count">
          {completed}/{total}
        </span>
      </div>
      <div className="subagent-list">
        {Array.from({ length: completed }).map((_, i) => (
          <div key={`completed-${i}`} className="subagent-item completed">
            <span className="subagent-icon">✓</span>
            <span className="subagent-label">Sub-agent {i + 1}</span>
            <span className="subagent-status">completed</span>
          </div>
        ))}
        {Array.from({ length: pending }).map((_, i) => (
          <div key={`pending-${i}`} className="subagent-item pending">
            <span className="subagent-icon">
              <span className="spinner"></span>
            </span>
            <span className="subagent-label">Sub-agent {completed + i + 1}</span>
            <span className="subagent-status">running...</span>
          </div>
        ))}
      </div>
    </div>
  );
}
