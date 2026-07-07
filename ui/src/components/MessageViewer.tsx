import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { Message } from '../api';
import { useMessageReviewNotesData, useReviewNotesCommands } from '../contexts/ReviewNotesContext';
import type { ReviewNote } from '../contexts/ReviewNotesContext';
import { getMessageMarkdown } from '../utils/messageCopy';
import { ViewerShell } from './viewer/ViewerShell';
import { MarkdownViewerBody } from './viewer/MarkdownViewerBody';
import { NotesPanel } from './viewer/NotesPanel';
import { AnnotationDialog } from './viewer/AnnotationDialog';
import { CopyButton } from './CopyButton';
import { formatNotesForSend } from './viewer/formatNotes';
import { useRegisterFocusScope } from '../hooks/useFocusScope';

interface MessageViewerProps {
  sequenceId: number;
  messages: Message[];
  onClose: () => void;
  onSendNotes: (notes: string) => void;
  inline?: boolean | undefined;
}

export function MessageViewer({ sequenceId, messages, onClose, onSendNotes, inline }: MessageViewerProps) {
  useRegisterFocusScope('message-viewer');
  const message = useMemo(
    () => messages.find((m) => m.sequence_id === sequenceId) ?? null,
    [messages, sequenceId],
  );

  const content = message ? getMessageMarkdown(message) : '';
  const title = message ? messageTitle(message) : `Message #${sequenceId}`;
  const notes = useMessageReviewNotes(sequenceId, message?.message_id, onSendNotes);
  const lineRefs = useRef<Map<number, HTMLElement>>(new Map());
  const lineRefsSequenceId = useRef(sequenceId);
  if (lineRefsSequenceId.current !== sequenceId) {
    lineRefs.current.clear();
    lineRefsSequenceId.current = sequenceId;
  }

  const registerLineRef = useCallback((lineNumber: number, el: HTMLElement | null) => {
    if (el) lineRefs.current.set(lineNumber, el);
    else lineRefs.current.delete(lineNumber);
  }, []);

  const handleJumpTo = useCallback(
    (note: ReviewNote) => {
      if (note.anchor.kind !== 'message' || note.anchor.sequenceId !== sequenceId) return;
      const el = lineRefs.current.get(note.anchor.lineNumber);
      if (el) {
        el.scrollIntoView({ behavior: 'smooth', block: 'center' });
        notes.highlight(note.anchor.lineNumber);
      }
      notes.closePanel();
    },
    [notes, sequenceId],
  );

  const headerExtras = message && content ? (
    <CopyButton text={content} className="viewer-shell-copy-btn" title="Copy message markdown" />
  ) : null;

  return (
    <ViewerShell
      mode={inline ? 'inline' : 'overlay'}
      ariaLabel={`Message viewer: ${title}`}
      title={title}
      titleTooltip={message ? `Conversation message #${sequenceId}` : undefined}
      headerExtras={headerExtras}
      noteCount={notes.messageNotes.length}
      onToggleNotes={notes.togglePanel}
      onSend={notes.send}
      onClose={onClose}
      bodyScroll="shell"
      panel={
        notes.showPanel ? (
          <NotesPanel
            notes={notes.messageNotes}
            onJumpTo={handleJumpTo}
            onRemove={notes.removeNote}
            onClearAll={notes.clearAll}
            onSend={notes.send}
            onClose={notes.closePanel}
          />
        ) : null
      }
      dialog={
        notes.annotating ? (
          <AnnotationDialog
            anchorLabel={`Line ${notes.annotating.lineNumber}`}
            lineContent={notes.annotating.lineContent}
            onSubmit={notes.submitNote}
            onCancel={notes.cancelAnnotate}
          />
        ) : null
      }
    >
      <div className="viewer-content">
        {message && content ? (
          <MarkdownViewerBody
            content={content}
            modifiedLines={EMPTY_SET}
            highlightedLine={notes.highlightedLine}
            onAnnotate={notes.startAnnotate}
            registerLineRef={registerLineRef}
          />
        ) : (
          <div className="viewer-error">
            <span>{message ? 'This message has no markdown content to annotate.' : 'Message not found.'}</span>
            <button onClick={onClose}>Close</button>
          </div>
        )}
      </div>
    </ViewerShell>
  );
}

const EMPTY_SET: Set<number> = new Set();

function messageTitle(message: Message): string {
  const type = message.message_type || (message as unknown as Record<string, unknown>)['type'];
  const label = type === 'agent' ? 'Agent' : type === 'user' ? 'User' : 'Message';
  return `${label} message #${message.sequence_id}`;
}

function useMessageReviewNotes(
  sequenceId: number,
  messageId: string | undefined,
  onSendNotes: (notes: string) => void,
) {
  const commands = useReviewNotesCommands();
  const messageNotes = useMessageReviewNotesData(sequenceId);
  const [annotating, setAnnotating] = useState<{ sequenceId: number; lineNumber: number; lineContent: string } | null>(null);
  const [showPanel, setShowPanel] = useState(false);
  const [highlightedLine, setHighlightedLine] = useState<number | null>(null);

  useEffect(() => {
    if (highlightedLine === null) return undefined;
    const timer = setTimeout(() => setHighlightedLine(null), 2000);
    return () => clearTimeout(timer);
  }, [highlightedLine]);

  useEffect(() => {
    setAnnotating(null);
    setHighlightedLine(null);
  }, [sequenceId]);

  const startAnnotate = useCallback((lineNumber: number, lineContent: string) => {
    setAnnotating({ sequenceId, lineNumber, lineContent });
  }, [sequenceId]);
  const cancelAnnotate = useCallback(() => setAnnotating(null), []);
  const submitNote = useCallback(
    (body: string) => {
      if (!annotating || annotating.sequenceId !== sequenceId) return;
      commands.addNote(
        {
          kind: 'message',
          sequenceId,
          ...(messageId !== undefined ? { messageId } : {}),
          lineNumber: annotating.lineNumber,
        },
        annotating.lineContent,
        body,
      );
      setAnnotating(null);
    },
    [annotating, commands, messageId, sequenceId],
  );

  const send = useCallback(() => {
    const formatted = formatNotesForSend(commands.getSnapshot());
    if (formatted) {
      onSendNotes(formatted);
      commands.clear();
      setShowPanel(false);
    }
  }, [commands, onSendNotes]);

  const togglePanel = useCallback(() => setShowPanel((v) => !v), []);
  const closePanel = useCallback(() => setShowPanel(false), []);
  const highlight = useCallback((lineNumber: number) => setHighlightedLine(lineNumber), []);
  const clearAll = useCallback(() => {
    commands.clear();
    setShowPanel(false);
  }, [commands]);

  return {
    messageNotes,
    annotating,
    startAnnotate,
    cancelAnnotate,
    submitNote,
    showPanel,
    togglePanel,
    closePanel,
    highlightedLine,
    highlight,
    send,
    clearAll,
    removeNote: commands.removeNote,
  };
}
