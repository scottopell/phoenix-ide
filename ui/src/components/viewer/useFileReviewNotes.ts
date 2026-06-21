import { useCallback, useEffect, useState } from 'react';
import {
  useFileReviewNotesData,
  useReviewNotesCommands,
} from '../../contexts/ReviewNotesContext';
import type { ReviewNote } from '../../contexts/ReviewNotesContext';
import { formatNotesForSend } from './formatNotes';
import type { PatchContext } from './metaViewerTypes';

/**
 * File-scoped review-note lifecycle, shared by every text-like viewer body
 * through MetaViewer. Owns the annotation dialog target, the notes panel
 * visibility, and the transient jump-highlight, plus the add/send/clear
 * operations against the conversation-scoped `ReviewNotesContext`.
 *
 * It deliberately does NOT own DOM line refs or scrolling — that is the
 * viewer's job, since the refs are created by the rendered body. The hook
 * exposes `highlight(lineNumber)` so the viewer can flash a line after it
 * scrolls to it.
 *
 * The diff viewer has a parallel `useDiffReviewNotes` over the same context;
 * the two anchor families (file vs diff/diff-file) are different enough that a
 * single generic hook would be a forced abstraction.
 */
export interface FileReviewNotes {
  /** Notes anchored to this file, for the panel and per-file count. */
  fileNotes: ReviewNote[];
  /** The line currently targeted by the annotation dialog, or null. */
  annotating: { lineNumber: number; lineContent: string } | null;
  startAnnotate: (lineNumber: number, lineContent: string) => void;
  cancelAnnotate: () => void;
  submitNote: (body: string) => void;
  showPanel: boolean;
  togglePanel: () => void;
  closePanel: () => void;
  /** Line to flash; cleared automatically after the highlight animation. */
  highlightedLine: number | null;
  highlight: (lineNumber: number) => void;
  /** Format the entire pile, hand it to the host, and clear. No-op if empty. */
  send: () => void;
  clearAll: () => void;
  removeNote: (id: string) => void;
}

export function useFileReviewNotes(
  absolutePath: string,
  onSendNotes: (notes: string) => void,
  patchContext?: PatchContext | undefined,
): FileReviewNotes {
  const commands = useReviewNotesCommands();
  const [annotating, setAnnotating] = useState<
    { lineNumber: number; lineContent: string } | null
  >(null);
  const [showPanel, setShowPanel] = useState(false);
  const [highlightedLine, setHighlightedLine] = useState<number | null>(null);

  const fileNotes = useFileReviewNotesData(absolutePath);

  // Clear the jump highlight after the flash animation.
  useEffect(() => {
    if (highlightedLine === null) return undefined;
    const timer = setTimeout(() => setHighlightedLine(null), 2000);
    return () => clearTimeout(timer);
  }, [highlightedLine]);

  const startAnnotate = useCallback((lineNumber: number, lineContent: string) => {
    setAnnotating({ lineNumber, lineContent });
  }, []);

  const cancelAnnotate = useCallback(() => setAnnotating(null), []);

  const submitNote = useCallback(
    (body: string) => {
      if (!annotating) return;
      // Prefix notes on a patch-modified line so the agent sees the line was
      // part of its own change set.
      const isModified = patchContext?.modifiedLines.has(annotating.lineNumber);
      const finalBody = isModified && !body.startsWith('[Changed line]')
        ? `[Changed line] ${body}`
        : body;
      commands.addNote(
        { kind: 'file', filePath: absolutePath, lineNumber: annotating.lineNumber },
        annotating.lineContent,
        finalBody,
      );
      setAnnotating(null);
    },
    [annotating, absolutePath, patchContext?.modifiedLines, commands],
  );

  const togglePanel = useCallback(() => setShowPanel((v) => !v), []);
  const closePanel = useCallback(() => setShowPanel(false), []);
  const highlight = useCallback((lineNumber: number) => setHighlightedLine(lineNumber), []);

  const send = useCallback(() => {
    const formatted = formatNotesForSend(commands.getSnapshot());
    if (formatted) {
      onSendNotes(formatted);
      commands.clear();
      setShowPanel(false);
    }
  }, [commands, onSendNotes]);

  const clearAll = useCallback(() => {
    commands.clear();
    setShowPanel(false);
  }, [commands]);

  return {
    fileNotes,
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
