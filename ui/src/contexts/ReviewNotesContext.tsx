import {
  createContext,
  useContext,
  useRef,
  useState,
  useSyncExternalStore,
} from 'react';
import type { ReactNode } from 'react';
import { generateUUID } from '../utils/uuid';

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

/**
 * Stable command surface for the review-notes pile. The function identities
 * never change across the store's lifetime, so a command-only consumer placed
 * in {@link ReviewNotesCommandContext} never re-renders when the notes data
 * mutates. `getSnapshot` reads the whole pile imperatively (used by the send
 * path) without subscribing — calling it does not create a render dependency.
 */
export interface ReviewNotesCommands {
  addNote: (anchor: NoteAnchor, lineContent: string, body: string) => void;
  updateNote: (id: string, body: string) => void;
  removeNote: (id: string) => void;
  clear: () => void;
  /** Read the entire pile without subscribing — for one-shot reads like the
   *  send path that formats every note. Does not establish a render
   *  dependency on the notes data. */
  getSnapshot: () => ReviewNote[];
}

/**
 * External store for the review-notes pile. A single source of truth backs the
 * data so command consumers and per-scope selector consumers cannot diverge.
 * Subscribers attached via `useSyncExternalStore` are notified on every
 * mutation; each selector hook narrows that to its own scope so an unrelated
 * note family does not trigger a re-render.
 */
interface ReviewNotesStore {
  subscribe: (listener: () => void) => () => void;
  getNotes: () => ReviewNote[];
  commands: ReviewNotesCommands;
}

function createReviewNotesStore(): ReviewNotesStore {
  let notes: ReviewNote[] = [];
  const listeners = new Set<() => void>();

  const emit = () => {
    for (const l of listeners) l();
  };

  const setNotes = (next: ReviewNote[]) => {
    notes = next;
    emit();
  };

  const commands: ReviewNotesCommands = {
    addNote: (anchor, lineContent, body) => {
      setNotes([
        ...notes,
        {
          id: generateUUID(),
          anchor,
          lineContent,
          body,
          createdAt: Date.now(),
        },
      ]);
    },
    updateNote: (id, body) => {
      setNotes(notes.map((n) => (n.id === id ? { ...n, body } : n)));
    },
    removeNote: (id) => {
      setNotes(notes.filter((n) => n.id !== id));
    },
    clear: () => setNotes([]),
    getSnapshot: () => notes,
  };

  return {
    subscribe: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    getNotes: () => notes,
    commands,
  };
}

const ReviewNotesCommandContext = createContext<ReviewNotesCommands | null>(null);
const ReviewNotesStoreContext = createContext<ReviewNotesStore | null>(null);

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
 *
 * Data is held in an external store and consumed through narrow selector
 * hooks ({@link useFileReviewNotesData}, {@link useDiffReviewNotesData}) so a
 * mutation in one scope only re-renders consumers of that scope. Commands are
 * exposed separately ({@link useReviewNotesCommands}) with stable identity, so
 * a command-only consumer never re-renders on data changes.
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
  const storeRef = useRef<ReviewNotesStore | null>(null);
  if (storeRef.current === null) {
    storeRef.current = createReviewNotesStore();
  }

  // Reset the pile when the scope changes by swapping in a FRESH store during
  // render — never by mutating the current one. A mutator here would emit to the
  // previous scope's `useSyncExternalStore` subscribers mid-render (StrictMode
  // warns, and a concurrent retry could clear the old scope before the new one
  // commits). A fresh store starts empty; the context swap makes children
  // re-subscribe to it on commit, reading the empty snapshot for the new scope.
  const [trackedScope, setTrackedScope] = useState<string | undefined>(scopeKey);
  if (trackedScope !== scopeKey) {
    setTrackedScope(scopeKey);
    storeRef.current = createReviewNotesStore();
  }
  const store = storeRef.current;

  return (
    <ReviewNotesStoreContext.Provider value={store}>
      <ReviewNotesCommandContext.Provider value={store.commands}>
        {children}
      </ReviewNotesCommandContext.Provider>
    </ReviewNotesStoreContext.Provider>
  );
}

function useReviewNotesStore(): ReviewNotesStore {
  const store = useContext(ReviewNotesStoreContext);
  if (!store) {
    throw new Error(
      'review-notes hooks must be used inside <ReviewNotesProvider>. ' +
        'Wrap the conversation page in the provider.',
    );
  }
  return store;
}

/**
 * Stable command surface for the notes pile. A consumer of this hook never
 * re-renders when the notes data mutates — only the selector hooks below do.
 */
// eslint-disable-next-line react-refresh/only-export-components
export function useReviewNotesCommands(): ReviewNotesCommands {
  const commands = useContext(ReviewNotesCommandContext);
  if (!commands) {
    throw new Error(
      'useReviewNotesCommands must be used inside <ReviewNotesProvider>. ' +
        'Wrap the conversation page in the provider.',
    );
  }
  return commands;
}

/**
 * Subscribe to the notes for a single file path. Re-renders only when the
 * filtered slice for `absolutePath` changes — a note added to another path or
 * to the diff scope does not re-render this consumer. The returned array is
 * referentially stable while its contents are unchanged, so it is safe to use
 * directly as a dependency.
 */
// eslint-disable-next-line react-refresh/only-export-components
export function useFileReviewNotesData(absolutePath: string): ReviewNote[] {
  const store = useReviewNotesStore();
  const cacheRef = useRef<ReviewNote[]>([]);
  const getSnapshot = () => {
    const next = store
      .getNotes()
      .filter((n) => n.anchor.kind === 'file' && n.anchor.filePath === absolutePath);
    return sameNotes(cacheRef.current, next) ? cacheRef.current : (cacheRef.current = next);
  };
  return useSyncExternalStore(store.subscribe, getSnapshot, getSnapshot);
}

/**
 * Subscribe to the diff-scoped notes (both `diff` line anchors and `diff-file`
 * anchors). Re-renders only when that slice changes — a note added to a file
 * scope does not re-render this consumer. The returned array is referentially
 * stable while its contents are unchanged.
 */
// eslint-disable-next-line react-refresh/only-export-components
export function useDiffReviewNotesData(): ReviewNote[] {
  const store = useReviewNotesStore();
  const cacheRef = useRef<ReviewNote[]>([]);
  const getSnapshot = () => {
    const next = store
      .getNotes()
      .filter((n) => n.anchor.kind === 'diff' || n.anchor.kind === 'diff-file');
    return sameNotes(cacheRef.current, next) ? cacheRef.current : (cacheRef.current = next);
  };
  return useSyncExternalStore(store.subscribe, getSnapshot, getSnapshot);
}

/**
 * Reference-stable equality for a filtered notes slice: same length and same
 * element identities in order. Notes are immutable (mutations replace the
 * object), so identity comparison is sufficient to detect a changed slice.
 */
function sameNotes(a: ReviewNote[], b: ReviewNote[]): boolean {
  if (a === b) return true;
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i += 1) {
    if (a[i] !== b[i]) return false;
  }
  return true;
}
