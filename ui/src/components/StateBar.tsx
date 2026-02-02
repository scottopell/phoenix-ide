import { Link } from 'react-router-dom';
import type { Conversation, ConversationState } from '../api';
import { getStateDescription } from '../utils';

interface StateBarProps {
  conversation: Conversation | null;
  convState: string;
  stateData: ConversationState | null;
  eventSourceReady: boolean;
}

export function StateBar({ conversation, convState, stateData, eventSourceReady }: StateBarProps) {
  let dotClass = 'dot';
  let stateText = '';

  if (!conversation) {
    dotClass += ' hidden';
    stateText = '';
  } else if (!eventSourceReady) {
    dotClass += ' connecting';
    stateText = 'connecting...';
  } else if (convState === 'idle') {
    dotClass += ' idle';
    stateText = 'ready';
  } else if (convState === 'error') {
    dotClass += ' error';
    stateText = stateData?.message || 'error';
  } else {
    dotClass += ' working';
    stateText = getStateDescription(convState, stateData);
  }

  return (
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
  );
}
