import type { ReactNode } from 'react';
import './AssistantMessageChrome.css';

export function AssistantMessageChrome({ isFirstInTurn, metadata, actions }: {
  isFirstInTurn: boolean;
  metadata: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <div className={`assistant-message-toolbar${isFirstInTurn ? ' message-header' : ' assistant-message-toolbar--continuation'}`}>
      {isFirstInTurn && metadata}
      <span className="assistant-message-actions">{actions}</span>
    </div>
  );
}

export function AssistantMessageMetadata({ timestamp, formattedTime, children }: {
  timestamp: string | undefined;
  formattedTime: string;
  children?: ReactNode;
}) {
  return (
    <>
      <span className="message-sender">Phoenix</span>
      {timestamp && (
        <span className="message-time" title={new Date(timestamp).toLocaleString()}>
          {formattedTime}
        </span>
      )}
      {children}
    </>
  );
}
