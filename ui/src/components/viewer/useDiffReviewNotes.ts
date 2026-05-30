import { useCallback, useEffect, useMemo, useState } from 'react';
import { useReviewNotes } from '../../contexts/ReviewNotesContext';
import type { DiffSection, NoteAnchor, ReviewNote } from '../../contexts/ReviewNotesContext';
import { formatNotesForSend } from './formatNotes';
import type { DiffLine, DiffSegment } from './diffParse';

/**
 * Composite key from the section discriminator + per-section diff position, so
 * committed and uncommitted positions never collide. Shared with DiffView's
 * ref/highlight maps.
 */
export function diffKey(section: DiffSection, diffPos: number): string {
  return `${section}:${diffPos}`;
}

/** What an in-progress diff annotation targets: a single line or a whole file. */
export type AnnotateTarget =
  | { kind: 'line'; section: DiffSection; segment: DiffSegment; line: DiffLine }
  | { kind: 'file'; section: DiffSection; segment: DiffSegment; diffPos: number };

/**
 * Diff-scoped review-note lifecycle, the diff counterpart to
 * `useFileReviewNotes`. Owns the annotation target, panel visibility, and
 * jump highlight (keyed by `section:diffPos`), plus the add/send/clear
 * operations against the conversation-scoped `ReviewNotesContext`.
 *
 * The two hooks share the context and the send/clear semantics but diverge on
 * anchor shape — file notes carry an absolute path + line number, diff notes
 * carry a section + diffPos and a file-level variant — which is why they are
 * separate hooks rather than one parameterized over kind.
 *
 * Scrolling on jump stays in DiffView, which owns the line DOM refs; the hook
 * exposes `highlight(key)` to flash the line after the view scrolls to it.
 */
export interface DiffReviewNotes {
  diffNotes: ReviewNote[];
  annotating: AnnotateTarget | null;
  startAnnotateLine: (section: DiffSection, segment: DiffSegment, line: DiffLine) => void;
  startAnnotateFile: (section: DiffSection, segment: DiffSegment, diffPos: number) => void;
  cancelAnnotate: () => void;
  submitNote: (body: string) => void;
  showPanel: boolean;
  togglePanel: () => void;
  closePanel: () => void;
  highlightedKey: string | null;
  highlight: (key: string) => void;
  send: () => void;
  clearAll: () => void;
  removeNote: (id: string) => void;
}

export function useDiffReviewNotes(onSendNotes: (notes: string) => void): DiffReviewNotes {
  const reviewNotes = useReviewNotes();
  const [annotating, setAnnotating] = useState<AnnotateTarget | null>(null);
  const [showPanel, setShowPanel] = useState(false);
  const [highlightedKey, setHighlightedKey] = useState<string | null>(null);

  const diffNotes = useMemo(() => reviewNotes.notesForDiff(), [reviewNotes]);

  useEffect(() => {
    if (highlightedKey === null) return undefined;
    const timer = setTimeout(() => setHighlightedKey(null), 2000);
    return () => clearTimeout(timer);
  }, [highlightedKey]);

  const startAnnotateLine = useCallback(
    (section: DiffSection, segment: DiffSegment, line: DiffLine) =>
      setAnnotating({ kind: 'line', section, segment, line }),
    [],
  );
  const startAnnotateFile = useCallback(
    (section: DiffSection, segment: DiffSegment, diffPos: number) =>
      setAnnotating({ kind: 'file', section, segment, diffPos }),
    [],
  );
  const cancelAnnotate = useCallback(() => setAnnotating(null), []);

  const submitNote = useCallback(
    (body: string) => {
      if (!annotating) return;
      let anchor: NoteAnchor;
      let lineContent: string;
      if (annotating.kind === 'line') {
        anchor = {
          kind: 'diff',
          section: annotating.section,
          filePath: annotating.segment.filePath,
          newLine: annotating.line.newLine,
          oldLine: annotating.line.oldLine,
          diffPos: annotating.line.diffPos,
        };
        lineContent = annotating.line.text;
      } else {
        anchor = {
          kind: 'diff-file',
          section: annotating.section,
          filePath: annotating.segment.filePath,
          diffPos: annotating.diffPos,
        };
        lineContent = '';
      }
      reviewNotes.addNote(anchor, lineContent, body);
      setAnnotating(null);
    },
    [annotating, reviewNotes],
  );

  const togglePanel = useCallback(() => setShowPanel((v) => !v), []);
  const closePanel = useCallback(() => setShowPanel(false), []);
  const highlight = useCallback((key: string) => setHighlightedKey(key), []);

  const send = useCallback(() => {
    const formatted = formatNotesForSend(reviewNotes.notes);
    if (formatted) {
      onSendNotes(formatted);
      reviewNotes.clear();
      setShowPanel(false);
    }
  }, [reviewNotes, onSendNotes]);

  const clearAll = useCallback(() => {
    reviewNotes.clear();
    setShowPanel(false);
  }, [reviewNotes]);

  return {
    diffNotes,
    annotating,
    startAnnotateLine,
    startAnnotateFile,
    cancelAnnotate,
    submitNote,
    showPanel,
    togglePanel,
    closePanel,
    highlightedKey,
    highlight,
    send,
    clearAll,
    removeNote: reviewNotes.removeNote,
  };
}
