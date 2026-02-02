import { Link } from 'react-router-dom';
import type { Conversation, ConversationState } from '../api';
import type { ConnectionState } from '../hooks';
import { getStateDescription } from '../utils';

interface StateBarProps {
  conversation: Conversation | null;
  convState: string;
  stateData: ConversationState | null;
  connectionState: ConnectionState;
  connectionAttempt: number;
  nextRetryIn: number | null;
}

export function StateBar({
  conversation,
  convState,
  stateData,
  connectionState,
  connectionAttempt,
  nextRetryIn,
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

  return (
    <>
      <header id="state-bar">
        <div id="state-indicator">
          <span id="state-dot" className={dotClass}></span>
          <span id="state-text">{stateText}</span>
        </div>
        <div id="conversation-info">
          {conversation ? (
            <Link to="/" id="conv-slug">{conversation.slug}</Link>
          ) : (
            <span id="conv-slug">—</span>
          )}
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
