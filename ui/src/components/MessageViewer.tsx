import { useMemo } from 'react';
import type { Message } from '../api';
import { getMessageMarkdown } from '../utils/messageCopy';
import { useRegisterFocusScope } from '../hooks/useFocusScope';
import { MetaViewer } from './viewer/MetaViewer';
import { ViewerShell } from './viewer/ViewerShell';
import type { MarkdownViewerPayload } from './viewer/metaViewerTypes';

interface MessageViewerProps {
  sequenceId: number;
  messageId?: string | undefined;
  occurrenceToken?: string | undefined;
  messages: Message[];
  onClose: () => void;
  onSendNotes?: ((notes: string) => void | Promise<void>) | undefined;
  presentation?: 'pane' | 'fullscreen' | undefined;
  canTogglePresentation?: boolean | undefined;
  onPresentationChange?: ((presentation: 'pane' | 'fullscreen') => void) | undefined;
  inline?: boolean | undefined;
}

/**
 * MessageViewer — thin message-to-payload adapter over `MetaViewer`.
 *
 * Resolves canonical message Markdown from loaded conversation messages and
 * delegates shell chrome, Find, copy, Escape precedence, line navigation, and
 * review-note lifecycle to `MetaViewer`. Missing/empty states stay outside
 * `MetaViewer` so partial payloads are never fabricated.
 */
export function MessageViewer({
  sequenceId,
  messageId,
  occurrenceToken,
  messages,
  onClose,
  onSendNotes,
  presentation = 'pane',
  canTogglePresentation = false,
  onPresentationChange,
  inline,
}: MessageViewerProps) {
  const message = useMemo(() => {
    if (occurrenceToken) {
      return messages.find((m) => (m.display_data as { productOccurrenceToken?: string } | null | undefined)?.productOccurrenceToken === occurrenceToken)
        ?? null;
    }
    if (messageId) {
      return messages.find((m) => m.message_id === messageId) ?? null;
    }
    return messages.find((m) => m.sequence_id === sequenceId) ?? null;
  }, [messageId, messages, occurrenceToken, sequenceId]);

  const content = message ? getMessageMarkdown(message) : '';
  const title = message ? messageTitle(message) : `Message #${sequenceId}`;

  if (message && content) {
    const payload: MarkdownViewerPayload = {
      kind: 'markdown',
      source: {
        kind: 'message',
        sequenceId,
        ...(message.message_id || messageId ? { messageId: message.message_id ?? messageId } : {}),
        ...(occurrenceToken !== undefined ? { occurrenceToken } : {}),
      },
      title,
      content,
      onClose,
      ...(onSendNotes !== undefined ? { onSendNotes } : {}),
      presentation,
      canTogglePresentation,
      ...(onPresentationChange !== undefined ? { onPresentationChange } : {}),
      ...(inline !== undefined ? { inline } : {}),
    };
    return <MetaViewer payload={payload} />;
  }

  return (
    <EmptyMessageViewerShell
      title={title}
      message={message}
      sequenceId={sequenceId}
      inline={inline}
      onClose={onClose}
    />
  );
}

function EmptyMessageViewerShell({
  title,
  message,
  sequenceId,
  inline,
  onClose,
}: {
  title: string;
  message: Message | null;
  sequenceId: number;
  inline?: boolean | undefined;
  onClose: () => void;
}) {
  useRegisterFocusScope('message-viewer');
  return (
    <ViewerShell
      mode={inline ? 'inline' : 'overlay'}
      ariaLabel={`Message viewer: ${title}`}
      title={title}
      titleTooltip={message ? `Conversation message #${sequenceId}` : undefined}
      noteCount={0}
      onToggleNotes={() => undefined}
      onClose={onClose}
      bodyScroll="shell"
    >
      <div className="viewer-content">
        <div className="viewer-error">
          <span>{message ? 'This message has no markdown content to annotate.' : 'Message not found.'}</span>
          <button onClick={onClose}>Close</button>
        </div>
      </div>
    </ViewerShell>
  );
}

function messageTitle(message: Message): string {
  const type = message.message_type || (message as unknown as Record<string, unknown>)['type'];
  const label = type === 'agent' ? 'Agent' : type === 'user' ? 'User' : 'Message';
  return `${label} message #${message.sequence_id}`;
}
