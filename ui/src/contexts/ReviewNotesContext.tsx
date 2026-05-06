import { createContext, useCallback, useContext, useMemo, useState } from 'react';
import type { ReactNode } from 'react';
import { generateUUID } from '../utils/uuid';

/**
 * Which sub-section of the diff viewer a note was anchored in.
 * `diffPos` (the line index inside the unified diff text) restarts at
 * 0 for each section, so the section discriminator is required to
 * disambiguate notes — without it, position 5 in committed and
 * position 5 in uncommitted would collide on lookup.
 */
export type DiffSection = 'committed' | 'uncommitted';

/**
 * Anchor identifying where a review note attaches.
 *
 * `kind: 'file'` — note on a single file's line, addressed by absolute path
 * and 1-based line number.
 *
 * `kind: 'diff'` — note on a position in a unified diff. `filePath` is
 * extracted from the most recent `diff --git` header. `newLine` is the
 * post-change line number if computable; absent for `-` (deletion-only),
 * binary files, and file-level notes. `diffPos` is the line index within
 * the unified diff text — stable across UI re-renders. `section`
 * disambiguates the per-section position namespace.
 *
 * `kind: 'diff-file'` — file-level diff note (no line anchor; the user
 * is commenting on the whole file change). Also section-scoped.
 */
export type NoteAnchor =
  | { kind: 'file'; filePath: string; lineNumber: number }
  | {
      kind: 'diff';
      section: DiffSection;
      filePath: string;
      newLine?: number | undefined;
      oldLine?: number | undefined;
      diffPos: number;
    }
  | {
      kind: 'diff-file';
      section: DiffSection;
      filePath: string;
      diffPos: number;
    };

export interface ReviewNote {
  id: string;
  anchor: NoteAnchor;
  /** The line of source the note refers to (empty for file-level). Stored
   *  so the formatted send-to-LLM message can quote the line even after
   *  the underlying file/diff has changed. */
  lineContent: string;
  body: string;
  createdAt: number;
}

interface ReviewNotesValue {
  notes: ReviewNote[];
  addNote: (anchor: NoteAnchor, lineContent: string, body: string) => void;
  updateNote: (id: string, body: string) => void;
  removeNote: (id: string) => void;
  clear: () => void;
  /** Notes filtered to a specific anchor scope — file path for file
   *  viewers, "diff" for the diff viewer. Used by viewer components so
   *  each only shows the notes relevant to its scope (the global Send
   *  drops the entire pile). */
  notesForFile: (absolutePath: string) => ReviewNote[];
  notesForDiff: () => ReviewNote[];
}

const ReviewNotesContext = createContext<ReviewNotesValue | null>(null);

/**
 * Provider for the per-conversation review-notes pile.
 *
 * Hard requirement (per Scott): notes must survive close-and-reopen of
 * any viewer until the user explicitly sends or clears. The provider
 * lives at the conversation route level, so notes persist across
 * multiple file/diff viewer sessions within the same conversation.
 *
 * Notes are NOT persisted across page reloads or conversation switches
 * — that's a deliberate scope (a "review session" is bounded by the
 * conversation visit).
 */
export function ReviewNotesProvider({
  children,
  scopeKey,
}: {
  children: ReactNode;
  /**
   * Scope identifier (typically the active conversation slug). When this
   * changes, the notes pile is cleared — a review session is bounded by the
   * conversation visit (matching the docstring above), so navigating to a
   * different conversation must not carry notes across.
   */
  scopeKey?: string | undefined;
}) {
  const [notes, setNotes] = useState<ReviewNote[]>([]);
  const [trackedScope, setTrackedScope] = useState<string | undefined>(scopeKey);

  if (trackedScope !== scopeKey) {
    // Synchronous reset (adjust state during render). Children never see
    // notes from the previous scope.
    setTrackedScope(scopeKey);
    if (notes.length > 0) setNotes([]);
  }

  const addNote = useCallback(
    (anchor: NoteAnchor, lineContent: string, body: string) => {
      setNotes((prev) => [
        ...prev,
        {
          id: generateUUID(),
          anchor,
          lineContent,
          body,
          createdAt: Date.now(),
        },
      ]);
    },
    [],
  );

  const updateNote = useCallback((id: string, body: string) => {
    setNotes((prev) => prev.map((n) => (n.id === id ? { ...n, body } : n)));
  }, []);

  const removeNote = useCallback((id: string) => {
    setNotes((prev) => prev.filter((n) => n.id !== id));
  }, []);

  const clear = useCallback(() => setNotes([]), []);

  const notesForFile = useCallback(
    (absolutePath: string) =>
      notes.filter(
        (n) => n.anchor.kind === 'file' && n.anchor.filePath === absolutePath,
      ),
    [notes],
  );

  const notesForDiff = useCallback(
    () =>
      notes.filter(
        (n) => n.anchor.kind === 'diff' || n.anchor.kind === 'diff-file',
      ),
    [notes],
  );

  const value = useMemo<ReviewNotesValue>(
    () => ({
      notes,
      addNote,
      updateNote,
      removeNote,
      clear,
      notesForFile,
      notesForDiff,
    }),
    [notes, addNote, updateNote, removeNote, clear, notesForFile, notesForDiff],
  );

  return (
    <ReviewNotesContext.Provider value={value}>
      {children}
    </ReviewNotesContext.Provider>
  );
}

// eslint-disable-next-line react-refresh/only-export-components
export function useReviewNotes(): ReviewNotesValue {
  const ctx = useContext(ReviewNotesContext);
  if (!ctx) {
    throw new Error(
      'useReviewNotes must be used inside <ReviewNotesProvider>. ' +
        'Wrap the conversation page in the provider.',
    );
  }
  return ctx;
}
