import { useCallback, useEffect, useState } from 'react';
import {
  useDiffReviewNotesData,
  useReviewNotesCommands,
} from '../../contexts/ReviewNotesContext';
import type { DiffSection, NoteAnchor, ReviewNote } from '../../contexts/ReviewNotesContext';
import { formatNotesForSend } from './formatNotes';

/** A line the user is about to annotate. Side identity is carried as the
 *  optional `newLine` (addition/context) or `oldLine` (deletion) — exactly one
 *  is set — matching the Pierre annotation side the note maps to. */
export interface LineAnnotateTarget {
  kind: 'line';
  section: DiffSection;
  filePath: string;
  newLine?: number | undefined;
  oldLine?: number | undefined;
  /** Raw line text, quoted into the formatted note. Recovered from typed
   *  Pierre hunk data, never DOM-scraped. */
  lineContent: string;
}

/** A whole-file (header) annotation target. */
export interface FileAnnotateTarget {
  kind: 'file';
  section: DiffSection;
  filePath: string;
}

/** What an in-progress diff annotation targets: a single line or a whole file. */
export type AnnotateTarget = LineAnnotateTarget | FileAnnotateTarget;

/**
 * Diff-scoped review-note lifecycle, the diff counterpart to
 * `useFileReviewNotes`. Owns the annotation target, panel visibility, and the
 * jump highlight (keyed by note id), plus the add/send/clear operations against
 * the conversation-scoped `ReviewNotesContext`.
 *
 * The two hooks share the context and the send/clear semantics but diverge on
 * anchor shape — file notes carry an absolute path + line number, diff notes
 * carry a section + side/line number and a file-level variant — which is why
 * they are separate hooks rather than one parameterized over kind.
 *
 * Scrolling on jump lives in the Pierre wrapper, which holds the typed
 * `CodeView` handle; the hook exposes `highlight(noteId)` to flash the note's
 * annotation after the view scrolls to it.
 */
export interface DiffReviewNotes {
  diffNotes: ReviewNote[];
  annotating: AnnotateTarget | null;
  startAnnotateLine: (target: Omit<LineAnnotateTarget, 'kind'>) => void;
  startAnnotateFile: (section: DiffSection, filePath: string) => void;
  cancelAnnotate: () => void;
  submitNote: (body: string) => void;
  showPanel: boolean;
  togglePanel: () => void;
  closePanel: () => void;
  highlightedNoteId: string | null;
  highlight: (noteId: string) => void;
  send: () => void;
  clearAll: () => void;
  removeNote: (id: string) => void;
}

export function useDiffReviewNotes(onSendNotes: (notes: string) => void): DiffReviewNotes {
  const commands = useReviewNotesCommands();
  const [annotating, setAnnotating] = useState<AnnotateTarget | null>(null);
  const [showPanel, setShowPanel] = useState(false);
  const [highlightedNoteId, setHighlightedNoteId] = useState<string | null>(null);

  const diffNotes = useDiffReviewNotesData();

  useEffect(() => {
    if (highlightedNoteId === null) return undefined;
    const timer = setTimeout(() => setHighlightedNoteId(null), 2000);
    return () => clearTimeout(timer);
  }, [highlightedNoteId]);

  const startAnnotateLine = useCallback(
    (target: Omit<LineAnnotateTarget, 'kind'>) => setAnnotating({ kind: 'line', ...target }),
    [],
  );
  const startAnnotateFile = useCallback(
    (section: DiffSection, filePath: string) => setAnnotating({ kind: 'file', section, filePath }),
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
          filePath: annotating.filePath,
          newLine: annotating.newLine,
          oldLine: annotating.oldLine,
        };
        lineContent = annotating.lineContent;
      } else {
        anchor = {
          kind: 'diff-file',
          section: annotating.section,
          filePath: annotating.filePath,
        };
        lineContent = '';
      }
      commands.addNote(anchor, lineContent, body);
      setAnnotating(null);
    },
    [annotating, commands],
  );

  const togglePanel = useCallback(() => setShowPanel((v) => !v), []);
  const closePanel = useCallback(() => setShowPanel(false), []);
  const highlight = useCallback((noteId: string) => setHighlightedNoteId(noteId), []);

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
    diffNotes,
    annotating,
    startAnnotateLine,
    startAnnotateFile,
    cancelAnnotate,
    submitNote,
    showPanel,
    togglePanel,
    closePanel,
    highlightedNoteId,
    highlight,
    send,
    clearAll,
    removeNote: commands.removeNote,
  };
}
