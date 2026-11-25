import { useCallback, useEffect, useRef, useState } from 'react';
import {
  useDocumentReviewNotesData,
  useReviewNotesCommands,
} from '../../contexts/ReviewNotesContext';
import type { ReviewNote } from '../../contexts/ReviewNotesContext';
import { formatNotesForSend } from './formatNotes';
import { isFileTextSource, viewerSourceIdentity, type ViewerSource } from './metaViewerTypes';

/**
 * Source-aware review-note lifecycle shared by all resolved MetaViewer payloads
 * (both filesystem documents and conversation messages).
 *
 * File and message anchors remain structurally distinct (`kind: 'file'` vs
 * `kind: 'message'`). Callback replacement keeps `startAnnotate` referentially
 * stable so rendered bodies (e.g. Mermaid diagrams) never remount when the host
 * recreates `onSendNotes`.
 */
export interface DocumentReviewNotes {
  /** Notes anchored to this source, for the panel and badge count. */
  notes: ReviewNote[];
  /** The line currently targeted by the annotation dialog, or null. */
  annotating: { sourceKey: string; lineNumber: number; lineContent: string } | null;
  annotationDraftDirty: boolean;
  setAnnotationDraftDirty: (dirty: boolean) => void;
  startAnnotate: (lineNumber: number, lineContent: string) => void;
  cancelAnnotate: () => void;
  submitNote: (body: string) => void;
  showPanel: boolean;
  togglePanel: () => void;
  closePanel: () => void;
  /** Line to flash; cleared automatically after the highlight animation. */
  highlightedLine: number | null;
  highlight: (lineNumber: number) => void;
  /** Format the entire pile, hand it to the host, and clear after success. */
  send: () => Promise<void>;
  clearAll: () => void;
  removeNote: (id: string) => void;
}

export function useDocumentReviewNotes(
  source: ViewerSource,
  onSendNotes: ((notes: string) => void | Promise<void>) | undefined,
): DocumentReviewNotes {
  const commands = useReviewNotesCommands();
  const sourceKey = viewerSourceIdentity(source);
  const notes = useDocumentReviewNotesData(source);

  const [annotating, setAnnotating] = useState<
    { sourceKey: string; lineNumber: number; lineContent: string } | null
  >(null);
  const [annotationDraftDirty, setAnnotationDraftDirty] = useState(false);
  const [showPanel, setShowPanel] = useState(false);
  const [highlightedLine, setHighlightedLine] = useState<number | null>(null);

  const onSendNotesRef = useRef(onSendNotes);
  onSendNotesRef.current = onSendNotes;
  const canAnnotate = Boolean(onSendNotes);

  // Clear the jump highlight after the flash animation.
  useEffect(() => {
    if (highlightedLine === null) return undefined;
    const timer = setTimeout(() => setHighlightedLine(null), 2000);
    return () => clearTimeout(timer);
  }, [highlightedLine]);

  // Switching document identity resets transient annotation/panel/highlight state.
  useEffect(() => {
    setAnnotating(null);
    setAnnotationDraftDirty(false);
    setHighlightedLine(null);
    setShowPanel(false);
  }, [sourceKey]);

  // If composer capability disappears, close the panel and clear highlights while
  // preserving an in-progress annotation draft so the close guard can protect it.
  useEffect(() => {
    if (canAnnotate) return;
    setHighlightedLine(null);
    setShowPanel(false);
  }, [canAnnotate]);

  const startAnnotate = useCallback((lineNumber: number, lineContent: string) => {
    if (!canAnnotate) return;
    setAnnotating({ sourceKey, lineNumber, lineContent });
  }, [canAnnotate, sourceKey]);

  const cancelAnnotate = useCallback(() => {
    setAnnotating(null);
    setAnnotationDraftDirty(false);
  }, []);

  const patchModifiedLines = isFileTextSource(source) ? source.patchContext?.modifiedLines : undefined;

  const submitNote = useCallback(
    (body: string) => {
      if (!annotating || annotating.sourceKey !== sourceKey) return;
      if (source.kind === 'file') {
        const isModified = patchModifiedLines?.has(annotating.lineNumber);
        const finalBody = isModified && !body.startsWith('[Changed line]')
          ? `[Changed line] ${body}`
          : body;
        commands.addNote(
          { kind: 'file', filePath: source.absolutePath, lineNumber: annotating.lineNumber },
          annotating.lineContent,
          finalBody,
        );
      } else {
        commands.addNote(
          {
            kind: 'message',
            sequenceId: source.sequenceId,
            ...(source.messageId !== undefined ? { messageId: source.messageId } : {}),
            ...(source.occurrenceToken !== undefined ? { occurrenceToken: source.occurrenceToken } : {}),
            lineNumber: annotating.lineNumber,
          },
          annotating.lineContent,
          body,
        );
      }
      setAnnotating(null);
      setAnnotationDraftDirty(false);
    },
    [annotating, commands, patchModifiedLines, source, sourceKey],
  );

  const togglePanel = useCallback(() => setShowPanel((v) => !v), []);
  const closePanel = useCallback(() => setShowPanel(false), []);
  const highlight = useCallback((lineNumber: number) => setHighlightedLine(lineNumber), []);

  const send = useCallback(async () => {
    const formatted = formatNotesForSend(commands.getSnapshot());
    const handler = onSendNotesRef.current;
    if (!formatted || !handler) return;
    await handler(formatted);
    commands.clear();
    setShowPanel(false);
  }, [commands]);

  const clearAll = useCallback(() => {
    commands.clear();
    setShowPanel(false);
  }, [commands]);

  return {
    notes,
    annotating,
    annotationDraftDirty,
    setAnnotationDraftDirty,
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
