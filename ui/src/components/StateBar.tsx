import { Link } from 'react-router-dom';
import type { Conversation, ConversationState } from '../api';
import type { ConnectionState } from '../hooks';
import { getStateDescription } from '../utils';

// Claude models all have 200k context window
const MAX_CONTEXT_TOKENS = 200_000;

interface StateBarProps {
  conversation: Conversation | null;
  convState: string;
  stateData: ConversationState | null;
  connectionState: ConnectionState;
  connectionAttempt: number;
  nextRetryIn: number | null;
  contextWindowUsed: number;
}

export function StateBar({
  conversation,
  convState,
  stateData,
  connectionState,
  connectionAttempt,
  nextRetryIn,
  contextWindowUsed,
}: StateBarProps) {
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
          stateText = stateData?.message || 'error';
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

  // Context window indicator
  const contextPercent = Math.min((contextWindowUsed / MAX_CONTEXT_TOKENS) * 100, 100);
  const contextWarning = contextPercent >= 80;
  const contextCritical = contextPercent >= 95;

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

  const tooltipText = `${formatTokens(contextWindowUsed)} / ${formatTokens(MAX_CONTEXT_TOKENS)} tokens (${contextPercent.toFixed(1)}%)`;

  return (
    <>
      <header id="state-bar">
        <div id="state-indicator">
          <span id="state-dot" className={dotClass}></span>
          <span id="state-text">{stateText}</span>
        </div>
        <div id="state-bar-right">
          {conversation && contextWindowUsed > 0 && (
            <div className={contextClass} title={tooltipText}>
              <div className="context-bar">
                <div 
                  className="context-fill" 
                  style={{ width: `${contextPercent}%` }}
                />
              </div>
              <span className="context-label">{formatTokens(contextWindowUsed)}</span>
            </div>
          )}
          <div id="conversation-info">
            {conversation ? (
              <Link to="/" id="conv-slug">{conversation.slug}</Link>
            ) : (
              <span id="conv-slug">—</span>
            )}
          </div>
        </div>
      </header>
      {showOfflineBanner && (
        <div className="offline-banner">
          <span className="offline-banner-icon">📡</span>
          <span className="offline-banner-text">
            Connection lost. Reconnecting in {nextRetryIn}s...
          </span>
        </div>
      )}
    </>
  );
}
