// VirtualizedMessageList.tsx
// Uses react-window for efficient rendering of large conversation histories.
// For message rendering components, see MessageComponents.tsx

import { useEffect, useRef, useState, useCallback, useMemo, CSSProperties } from 'react';
import { VariableSizeList } from 'react-window';
import type { Message, ToolResultContent, ConversationState } from '../api';
import type { QueuedMessage } from '../hooks';
import {
  UserMessage,
  QueuedUserMessage,
  AgentMessage,
  SubAgentStatus,
} from './MessageComponents';

// Types for the item data passed through react-window
interface ItemType {
  type: 'message' | 'queued' | 'subagent';
  data: Message | QueuedMessage | ConversationState;
}

interface RowData {
  items: ItemType[];
  toolResults: Map<string, Message>;
  onRetry: (localId: string) => void;
  onOpenFile?: (filePath: string, modifiedLines: Set<number>, firstModifiedLine: number) => void;
  setItemHeight: (index: number, height: number) => void;
}

// Define proper types for react-window
interface ListChildComponentProps {
  index: number;
  style: CSSProperties;
  data: RowData;
}

interface MessageListProps {
  messages: Message[];
  queuedMessages: QueuedMessage[];
  convState: string;
  stateData: ConversationState | null;
  onRetry: (localId: string) => void;
  onOpenFile?: (filePath: string, modifiedLines: Set<number>, firstModifiedLine: number) => void;
}

// Row component extracted to avoid hooks-in-conditional issue
function RowRenderer({ index, style, data }: ListChildComponentProps) {
  const item = data.items[index];
  const rowRef = useRef<HTMLDivElement>(null);
  
  // Measure height after render
  useEffect(() => {
    if (rowRef.current) {
      const height = rowRef.current.getBoundingClientRect().height;
      data.setItemHeight(index, height);
    }
  });

  if (!item) return null;

  return (
    <div ref={rowRef} style={style}>
      {item.type === 'message' && (() => {
        const msg = item.data as Message;
        const msgType = msg.message_type || msg.type;
        return msgType === 'user' ? (
          <UserMessage message={msg} />
        ) : (
          <AgentMessage message={msg} toolResults={data.toolResults} onOpenFile={data.onOpenFile} />
        );
      })()}
      {item.type === 'queued' && (
        <QueuedUserMessage message={item.data as QueuedMessage} onRetry={data.onRetry} />
      )}
      {item.type === 'subagent' && (
        <SubAgentStatus stateData={item.data as ConversationState} />
      )}
    </div>
  );
}

export function VirtualizedMessageList({ messages, queuedMessages, convState, stateData, onRetry, onOpenFile }: MessageListProps) {
  const mainRef = useRef<HTMLElement>(null);
  const listRef = useRef<VariableSizeList>(null);
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
    const result: ItemType[] = [];
    
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

  const itemData: RowData = {
    items,
    toolResults,
    onRetry,
    onOpenFile,
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
            {RowRenderer}
          </VariableSizeList>
        </div>
      </section>
    </main>
  );
}
