import { useState, useRef, useEffect } from 'react';
import { Link } from 'react-router-dom';
import type { Conversation, ConversationState } from '../api';
import type { ConnectionState } from '../hooks';
import { getStateDescription } from '../utils';

// Thresholds (match backend constants)
const WARNING_THRESHOLD = 0.80;
const CONTINUATION_THRESHOLD = 0.90;

interface StateBarProps {
  conversation: Conversation | null;
  convState: string;
  stateData: ConversationState | null;
  connectionState: ConnectionState;
  connectionAttempt: number;
  nextRetryIn: number | null;
  contextWindowUsed: number;
  /** Model's maximum context window in tokens */
  modelContextWindow: number;
  onRetryNow?: () => void;
  /** Callback to manually trigger continuation */
  onTriggerContinuation?: () => void;
}

export function StateBar({
  conversation,
  convState,
  stateData,
  connectionState,
  connectionAttempt,
  nextRetryIn,
  contextWindowUsed,
  modelContextWindow,
  onRetryNow,
  onTriggerContinuation,
}: StateBarProps) {
  const [menuOpen, setMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);

  // Close menu on outside click
  useEffect(() => {
    if (!menuOpen) return;
    const handleClick = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setMenuOpen(false);
      }
    };
    document.addEventListener('mousedown', handleClick);
    return () => document.removeEventListener('mousedown', handleClick);
  }, [menuOpen]);

  let dotClass = 'dot';
  let stateText = '';

  if (!conversation) {
    dotClass += ' hidden';
    stateText = '';
  } else {
    // Determine dot and text based on connection state first
    switch (connectionState) {
      case 'disconnected':
        dotClass += ' connecting';
        stateText = 'connecting...';
        break;

      case 'connecting':
        dotClass += ' connecting';
        stateText = 'connecting...';
        break;

      case 'reconnecting':
        dotClass += ' reconnecting';
        stateText = `reconnecting (${connectionAttempt})...`;
        break;

      case 'offline':
        dotClass += ' offline';
        stateText = 'offline';
        break;

      case 'reconnected':
        dotClass += ' reconnected';
        stateText = 'reconnected';
        break;

      case 'connected':
        // When connected, show agent state
        if (convState === 'idle') {
          dotClass += ' idle';
          stateText = 'ready';
        } else if (convState === 'error') {
          dotClass += ' error';
          stateText = 'error';
        } else if (convState === 'context_exhausted') {
          dotClass += ' error';
          stateText = 'context full';
        } else if (convState === 'awaiting_continuation') {
          dotClass += ' working';
          stateText = 'summarizing...';
        } else {
          dotClass += ' working';
          stateText = getStateDescription(convState, stateData);
        }
        break;

      default:
        dotClass += ' connecting';
        stateText = 'connecting...';
    }
  }

  const showOfflineBanner = connectionState === 'offline' && nextRetryIn !== null;

  // Context window indicator - use model-specific limit
  const maxTokens = modelContextWindow || 200_000; // Fallback for legacy
  const contextPercent = Math.min((contextWindowUsed / maxTokens) * 100, 100);
  const contextWarning = contextPercent / 100 >= WARNING_THRESHOLD;
  const contextCritical = contextPercent / 100 >= CONTINUATION_THRESHOLD;

  let contextClass = 'context-indicator';
  if (contextCritical) {
    contextClass += ' critical';
  } else if (contextWarning) {
    contextClass += ' warning';
  }

  const formatTokens = (n: number): string => {
    if (n >= 1000) {
      return `${(n / 1000).toFixed(0)}k`;
    }
    return n.toString();
  };

  const tooltipText = `${formatTokens(contextWindowUsed)} / ${formatTokens(maxTokens)} tokens (${contextPercent.toFixed(1)}%)`;

  // Format cwd for display - show last 2 path components
  const formatCwd = (cwd: string): string => {
    const parts = cwd.split('/').filter(Boolean);
    if (parts.length <= 2) return cwd;
    return '.../' + parts.slice(-2).join('/');
  };

  // Show menu trigger only when warning threshold reached and in idle state
  const canTriggerContinuation = contextWarning && convState === 'idle' && onTriggerContinuation;

  const handleTriggerContinuation = () => {
    setMenuOpen(false);
    onTriggerContinuation?.();
  };

  return (
    <>
      <header id="state-bar">
        <div id="state-bar-left">
          {conversation ? (
            <>
              <Link to="/" id="conv-slug" title="Back to conversations">
                <span className="back-arrow">←</span>
                {conversation.slug}
              </Link>
              <div className="conv-meta">
                <span className="conv-model" title={`Model: ${conversation.model}`}>
                  {conversation.model}
                </span>
                <span className="conv-separator">•</span>
                <span className="conv-cwd" title={conversation.cwd}>
                  {formatCwd(conversation.cwd)}
                </span>
              </div>
            </>
          ) : (
            <span id="conv-slug">—</span>
          )}
        </div>
        <div id="state-bar-right">
          <div id="state-indicator">
            <span id="state-dot" className={dotClass}></span>
            <span id="state-text">{stateText}</span>
          </div>
          {conversation && contextWindowUsed > 0 && (
            <div 
              className={contextClass} 
              title={tooltipText}
              ref={menuRef}
            >
              <div 
                className="context-bar-wrapper"
                onClick={() => canTriggerContinuation && setMenuOpen(!menuOpen)}
                style={{ cursor: canTriggerContinuation ? 'pointer' : 'default' }}
              >
                <div className="context-bar">
                  <div 
                    className="context-fill" 
                    style={{ width: `${contextPercent}%` }}
                  />
                </div>
                <span className="context-label">{formatTokens(contextWindowUsed)}</span>
                {canTriggerContinuation && (
                  <span className="context-menu-indicator">▼</span>
                )}
              </div>
              {menuOpen && canTriggerContinuation && (
                <div className="context-menu">
                  <button 
                    className="context-menu-item"
                    onClick={handleTriggerContinuation}
                  >
                    End & summarize conversation
                  </button>
                  <div className="context-menu-hint">
                    Creates a summary to continue in a new conversation
                  </div>
                </div>
              )}
            </div>
          )}
        </div>
      </header>
      {showOfflineBanner && (
        <div className="offline-banner">
          <span className="offline-banner-icon">📡</span>
          <span className="offline-banner-text">
            Connection lost. Reconnecting in {nextRetryIn}s...
          </span>
          {onRetryNow && (
            <button className="offline-banner-retry" onClick={onRetryNow}>
              Retry now
            </button>
          )}
        </div>
      )}
    </>
  );
}
