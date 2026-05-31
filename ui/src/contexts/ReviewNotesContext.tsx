import { createContext, useCallback, useContext, useMemo } from 'react';
import type { ReactNode } from 'react';
import { generateUUID } from '../utils/uuid';
import { useScopedState } from '../hooks/useScopedState';

/**
 * Which sub-section of the diff viewer a note was anchored in. The same file
 * can appear in both sections (committed and uncommitted), so the section
 * discriminator is required to disambiguate notes — it is also baked into the
 * Pierre `CodeView` item id (`${section}:${filePath}`) so the two can never
 * collide on lookup or jump.
 */
export type DiffSection = 'committed' | 'uncommitted';

/**
 * Anchor identifying where a review note attaches.
 *
 * `kind: 'file'` — note on a single file's line, addressed by absolute path
 * and 1-based line number.
 *
 * `kind: 'diff'` — note on a line in a unified diff, identified by
 * (`section`, `filePath`, side + line number): `newLine` for an addition or
 * context line, `oldLine` for a deletion. `diffPos` (the raw line index within
 * the unified diff text) is an optional legacy field — the Pierre renderer
 * identifies and jumps to lines by side + line number, never by `diffPos`. It
 * is retained only so notes carrying it continue to format/label correctly.
 *
 * `kind: 'diff-file'` — file-level diff note (no line anchor; the user is
 * commenting on the whole file change). Also section-scoped.
 */
export type NoteAnchor =
  | { kind: 'file'; filePath: string; lineNumber: number }
  | {
      kind: 'diff';
      section: DiffSection;
      filePath: string;
      newLine?: number | undefined;
      oldLine?: number | undefined;
      diffPos?: number | undefined;
    }
  | {
      kind: 'diff-file';
      section: DiffSection;
      filePath: string;
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
  const [notes, setNotes] = useScopedState<ReviewNote[]>(scopeKey, []);

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
    [setNotes],
  );

  const updateNote = useCallback((id: string, body: string) => {
    setNotes((prev) => prev.map((n) => (n.id === id ? { ...n, body } : n)));
  }, [setNotes]);

  const removeNote = useCallback((id: string) => {
    setNotes((prev) => prev.filter((n) => n.id !== id));
  }, [setNotes]);

  const clear = useCallback(() => setNotes([]), [setNotes]);

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
