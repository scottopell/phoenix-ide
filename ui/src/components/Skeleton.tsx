// Skeleton loading components

interface SkeletonProps {
  width?: string | number;
  height?: string | number;
  borderRadius?: string | number;
  className?: string;
  style?: React.CSSProperties;
}

export function Skeleton({ width, height, borderRadius = 4, className = '', style }: SkeletonProps) {
  return (
    <div
      className={`skeleton ${className}`}
      style={{
        width: typeof width === 'number' ? `${width}px` : width,
        height: typeof height === 'number' ? `${height}px` : height,
        borderRadius: typeof borderRadius === 'number' ? `${borderRadius}px` : borderRadius,
        ...style,
      }}
    />
  );
}

export function SkeletonText({ lines = 1, className = '' }: { lines?: number; className?: string }) {
  return (
    <div className={`skeleton-text ${className}`}>
      {Array.from({ length: lines }).map((_, i) => (
        <Skeleton
          key={i}
          height={14}
          width={i === lines - 1 ? '60%' : '100%'}
          className="skeleton-line"
        />
      ))}
    </div>
  );
}

// Conversation list item skeleton
export function ConversationItemSkeleton() {
  return (
    <div className="conv-item skeleton-item">
      <div className="conv-item-main">
        <Skeleton width="65%" height={18} className="skeleton-title" />
        <div className="conv-item-meta" style={{ marginTop: 8 }}>
          <Skeleton width={100} height={13} />
          <Skeleton width={50} height={13} />
        </div>
        <div className="conv-item-meta secondary" style={{ marginTop: 4 }}>
          <Skeleton width={80} height={12} />
          <Skeleton width={120} height={12} />
        </div>
      </div>
    </div>
  );
}

// Conversation list loading skeleton
export function ConversationListSkeleton({ count = 4 }: { count?: number }) {
  return (
    <div className="skeleton-list">
      {Array.from({ length: count }).map((_, i) => (
        <ConversationItemSkeleton key={i} />
      ))}
    </div>
  );
}

// Message skeleton for conversation page
export function MessageSkeleton({ isUser = false }: { isUser?: boolean }) {
  return (
    <div className={`message skeleton-message ${isUser ? 'user' : 'agent'}`}>
      <div className="message-header">
        <Skeleton width={isUser ? 30 : 55} height={12} />
        <Skeleton width={50} height={12} style={{ marginLeft: 8 }} />
      </div>
      <div className="message-content">
        <SkeletonText lines={isUser ? 1 : 3} />
      </div>
    </div>
  );
}

// Message list loading skeleton
export function MessageListSkeleton({ count = 3 }: { count?: number }) {
  return (
    <div className="skeleton-messages">
      {Array.from({ length: count }).map((_, i) => (
        <MessageSkeleton key={i} isUser={i % 2 === 0} />
      ))}
    </div>
  );
}
