// VirtualizedMessageList.tsx
import { useEffect, useRef, useState, useCallback, useMemo, CSSProperties } from 'react';
// @ts-ignore
import { VariableSizeList } from 'react-window';
import type { Message, ContentBlock, ToolResultContent, ConversationState } from '../api';
import type { QueuedMessage } from '../hooks';
import { escapeHtml, renderMarkdown } from '../utils';

interface MessageListProps {
  messages: Message[];
  queuedMessages: QueuedMessage[];
  convState: string;
  stateData: ConversationState | null;
  onRetry: (localId: string) => void;
}

interface RowData {
  messages: Message[];
  queuedMessages: QueuedMessage[];
  convState: string;
  stateData: ConversationState | null;
  toolResults: Map<string, Message>;
  onRetry: (localId: string) => void;
  getItemHeight: (index: number) => number;
  setItemHeight: (index: number, height: number) => void;
}

export function VirtualizedMessageList({ messages, queuedMessages, convState, stateData, onRetry }: MessageListProps) {
  const mainRef = useRef<HTMLElement>(null);
  const listRef = useRef<any>(null);
  const [mainHeight, setMainHeight] = useState(600);
  const itemHeights = useRef<Map<number, number>>(new Map());
  const scrollPositionRef = useRef<number>(0);
  const conversationIdRef = useRef<string>('');

  // Get conversation ID from the first message
  const conversationId = messages[0]?.conversation_id || '';
  if (conversationId && conversationId !== conversationIdRef.current) {
    conversationIdRef.current = conversationId;
    // Reset heights for new conversation
    itemHeights.current.clear();
  }

  // Build tool results map
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

  // Get sending messages
  const sendingMessages = useMemo(
    () => queuedMessages.filter(m => m.status === 'sending'),
    [queuedMessages]
  );

  // Combine all items to render
  const items = useMemo(() => {
    const result: Array<{ type: 'message' | 'queued' | 'subagent', data: any }> = [];
    
    // Add regular messages (skip tool messages)
    for (const msg of messages) {
      const type = msg.message_type || msg.type;
      if (type !== 'tool') {
        result.push({ type: 'message', data: msg });
      }
    }
    
    // Add queued messages
    for (const msg of sendingMessages) {
      result.push({ type: 'queued', data: msg });
    }
    
    // Add sub-agent status if active
    if (convState === 'awaiting_sub_agents' && stateData) {
      result.push({ type: 'subagent', data: stateData });
    }
    
    return result;
  }, [messages, sendingMessages, convState, stateData]);

  // Save scroll position before unmount
  useEffect(() => {
    return () => {
      if (conversationId && scrollPositionRef.current > 0) {
        sessionStorage.setItem(`scroll-${conversationId}`, scrollPositionRef.current.toString());
      }
    };
  }, [conversationId]);

  // Restore scroll position after mount
  useEffect(() => {
    if (conversationId && listRef.current && items.length > 0) {
      const savedPosition = sessionStorage.getItem(`scroll-${conversationId}`);
      if (savedPosition) {
        const position = parseInt(savedPosition, 10);
        listRef.current.scrollTo(position);
        sessionStorage.removeItem(`scroll-${conversationId}`);
      }
    }
  }, [conversationId, items.length]);

  // Update main area height on resize
  useEffect(() => {
    const updateHeight = () => {
      if (mainRef.current) {
        setMainHeight(mainRef.current.offsetHeight);
      }
    };
    
    updateHeight();
    window.addEventListener('resize', updateHeight);
    return () => window.removeEventListener('resize', updateHeight);
  }, []);

  // Scroll to bottom when new messages arrive
  useEffect(() => {
    if (listRef.current && items.length > 0) {
      listRef.current.scrollToItem(items.length - 1, 'end');
    }
  }, [items.length]);

  // Get/set item heights for dynamic sizing
  const getItemHeight = useCallback((index: number) => {
    return itemHeights.current.get(index) || 100; // Default estimate
  }, []);

  const setItemHeight = useCallback((index: number, height: number) => {
    if (itemHeights.current.get(index) !== height) {
      itemHeights.current.set(index, height);
      if (listRef.current) {
        listRef.current.resetAfterIndex(index);
      }
    }
  }, []);

  // Row renderer
  const Row = ({ index, style, data }: { index: number; style: CSSProperties; data: RowData }) => {
    const item = items[index];
    if (!item) return null;

    const rowRef = useRef<HTMLDivElement>(null);
    
    // Measure height after render
    useEffect(() => {
      if (rowRef.current) {
        const height = rowRef.current.getBoundingClientRect().height;
        data.setItemHeight(index, height);
      }
    });

    return (
      <div ref={rowRef} style={style}>
        {item.type === 'message' && (
          <>  
            {item.data.message_type === 'user' || item.data.type === 'user' ? (
              <UserMessage message={item.data} />
            ) : (
              <AgentMessage message={item.data} toolResults={data.toolResults} />
            )}
          </>
        )}
        {item.type === 'queued' && (
          <QueuedUserMessage message={item.data} onRetry={data.onRetry} />
        )}
        {item.type === 'subagent' && (
          <SubAgentStatus stateData={item.data} />
        )}
      </div>
    );
  };

  const itemData: RowData = {
    messages,
    queuedMessages: sendingMessages,
    convState,
    stateData,
    toolResults,
    onRetry,
    getItemHeight,
    setItemHeight,
  };

  if (items.length === 0) {
    return (
      <main id="main-area" ref={mainRef}>
        <section id="chat-view" className="view active">
          <div id="messages">
            <div className="empty-state">
              <div className="empty-state-icon">✨</div>
              <p>Start a conversation</p>
            </div>
          </div>
        </section>
      </main>
    );
  }

  return (
    <main id="main-area" ref={mainRef}>
      <section id="chat-view" className="view active">
        <div id="messages" style={{ height: '100%' }}>
          <VariableSizeList
            ref={listRef}
            height={mainHeight}
            itemCount={items.length}
            itemSize={getItemHeight}
            itemData={itemData}
            width="100%"
            overscanCount={3}
            onScroll={({ scrollOffset }: { scrollOffset: number }) => {
              scrollPositionRef.current = scrollOffset;
            }}
          >
            {Row}
          </VariableSizeList>
        </div>
      </section>
    </main>
  );
}

// Reuse the existing message components from MessageList.tsx
function UserMessage({ message }: { message: Message }) {
  const content = message.content as { text?: string; images?: { data: string; media_type: string }[] };
  const text = content.text || (typeof message.content === 'string' ? message.content : '');
  const images = content.images || [];

  return (
    <div className="message user">
      <div className="message-header">
        <span>You</span>
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

  return (
    <div className="message agent">
      <div className="message-header">Phoenix</div>
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

function ToolUseBlock({ block, result }: { block: ContentBlock; result?: Message }) {
  const [expanded, setExpanded] = useState(false);
  const name = block.name || 'tool';
  const input = block.input || {};
  const toolId = block.id || '';

  // Special handling for common tools
  let inputStr: string;
  if (name === 'bash' && input.command) {
    inputStr = String(input.command);
  } else if (name === 'think' && input.thoughts) {
    inputStr = String(input.thoughts);
  } else {
    inputStr = JSON.stringify(input, null, 2);
  }

  // Get the paired result if available
  let resultContent: ToolResultContent | null = null;
  if (result) {
    resultContent = result.content as ToolResultContent;
  }

  const resultText = resultContent?.content || resultContent?.result || resultContent?.error || '';
  const isError = resultContent?.is_error || !!resultContent?.error;

  // Truncate long results
  const maxLen = 500;
  const truncated = resultText.length > maxLen;
  const displayResult = truncated ? resultText.slice(0, maxLen) + '...' : resultText;

  return (
    <div className={`tool-group${expanded ? ' expanded' : ''}`} data-tool-id={toolId}>
      <div className="tool-header" onClick={() => setExpanded(!expanded)}>
        <span className="tool-name">{name}</span>
        <span className="tool-chevron">▶</span>
      </div>
      <div className="tool-body">
        <div className="tool-input">{inputStr}</div>
        {resultContent && (
          <div className={`tool-result-section${isError ? ' error' : ''}`}>
            <div className="tool-result-label">
              {isError ? '✗ error' : '✓ result'}
              {truncated && <span className="tool-truncated"> (truncated)</span>}
            </div>
            <div className="tool-result-content">
              {displayResult || <span className="tool-empty">(empty)</span>}
            </div>
          </div>
        )}
      </div>
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