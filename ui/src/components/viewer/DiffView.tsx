/**
 * DiffView — multi-file diff viewer with review-notes integration, rendered by
 * `@pierre/diffs` CodeView (via PhoenixDiffCodeView) on the shared `viewer/`
 * primitives.
 *
 * The committed and uncommitted diffs render as one virtualized CodeView
 * surface (committed files first); each file header carries a section badge and
 * a file-level note affordance. Lines are annotated via the gutter `+` or a
 * line click. Notes share the conversation-scoped pile with FileView; Send
 * drops the entire pile. Jump-to-line uses Pierre's typed scroll target.
 */

import { useCallback, useMemo, useRef, useState } from 'react';
import { Columns2, Rows3 } from 'lucide-react';
import type { DiffSection, ReviewNote } from '../../contexts/ReviewNotesContext';
import { useRegisterFocusScope } from '../../hooks/useFocusScope';
import {
  FindBar,
  buildDiffSearchProjection,
  useViewerFind,
  useViewerFindKeyboardShortcut,
  type DiffSearchMatchTarget,
} from '../viewer-find';
import { ViewerShell } from './ViewerShell';
import { NotesPanel } from './NotesPanel';
import { AnnotationDialog } from './AnnotationDialog';
import { useDiffReviewNotes } from './useDiffReviewNotes';
import type { AnnotateTarget } from './useDiffReviewNotes';
import { PhoenixDiffCodeView } from './PhoenixDiffCodeView';
import type { PhoenixDiffCodeViewHandle } from './PhoenixDiffCodeView';

export interface DiffViewProps {
  open: boolean;
  comparator: string;
  commitLog: string;
  committedDiff: string;
  committedTruncatedKib?: number | undefined;
  /** When true, `committedTruncatedKib` is a lower bound — render the
   *  truncation indicator with a "≥" prefix. */
  committedSaturated?: boolean | undefined;
  uncommittedDiff: string;
  uncommittedTruncatedKib?: number | undefined;
  uncommittedSaturated?: boolean | undefined;
  onClose: () => void;
  /** Drop the formatted review-notes pile into the chat input. Same
   *  signature as ProseReader's onSendNotes. */
  onSendNotes: (notes: string) => void;
  /** Render inline (no overlay) for desktop split-pane mode. */
  inline?: boolean;
  /** Render as a focused full-screen surface above app chrome. */
  takeover?: boolean;
}

type DiffStyle = 'unified' | 'split';
const DIFF_STYLE_KEY = 'phoenix-diff-style';

function initialDiffStyle(): DiffStyle {
  const stored = localStorage.getItem(DIFF_STYLE_KEY);
  return stored === 'split' ? 'split' : 'unified';
}

export function DiffView({
  open,
  comparator,
  commitLog,
  committedDiff,
  committedTruncatedKib,
  committedSaturated,
  uncommittedDiff,
  uncommittedTruncatedKib,
  uncommittedSaturated,
  onClose,
  onSendNotes,
  inline = false,
  takeover = false,
}: DiffViewProps) {
  useRegisterFocusScope('diff-viewer');
  const notes = useDiffReviewNotes(onSendNotes);
  const codeViewRef = useRef<PhoenixDiffCodeViewHandle>(null);
  const findButtonRef = useRef<HTMLButtonElement>(null);
  const findPreviousFocusRef = useRef<HTMLElement | null>(null);

  const [diffStyle, setDiffStyle] = useState<DiffStyle>(initialDiffStyle);
  const find = useViewerFind({ text: '' });

  const openFind = useCallback(() => {
    findPreviousFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    find.open();
  }, [find]);

  const closeFind = useCallback(() => {
    find.close();
    const restoreTarget = findPreviousFocusRef.current;
    queueMicrotask(() => (restoreTarget ?? findButtonRef.current)?.focus());
  }, [find]);

  useViewerFindKeyboardShortcut({
    scopeId: 'diff-viewer',
    onOpen: openFind,
    dialogOpen: notes.annotating !== null,
  });
  const findProjection = useMemo(
    () => (find.isOpen && find.query.length > 0
      ? buildDiffSearchProjection(committedDiff, uncommittedDiff, find.query, commitLog)
      : { sources: [], matches: [] }),
    [commitLog, committedDiff, find.isOpen, uncommittedDiff, find.query],
  );
  const activeFindIndex = findProjection.matches.length === 0
    ? -1
    : Math.min(Math.max(find.requestedActiveIndex, 0), findProjection.matches.length - 1);
  const activeFindMatchTarget = find.isOpen && activeFindIndex >= 0
    ? findProjection.matches[activeFindIndex]?.target ?? null
    : null;
  const findMatchTargets = useMemo(
    () => (find.isOpen ? findProjection.matches.map((match) => match.target) : []),
    [find.isOpen, findProjection.matches],
  );

  const toggleDiffStyle = useCallback(() => {
    setDiffStyle((prev) => {
      const next = prev === 'unified' ? 'split' : 'unified';
      localStorage.setItem(DIFF_STYLE_KEY, next);
      return next;
    });
  }, []);

  const diffNotes = notes.diffNotes;
  const { highlight, closePanel } = notes;

  // Jump uses Pierre's typed scroll target via the wrapper handle, then flashes
  // the note's annotation — no DOM scraping.
  const handleJumpTo = useCallback(
    (note: ReviewNote) => {
      if (note.anchor.kind !== 'diff' && note.anchor.kind !== 'diff-file') return;
      codeViewRef.current?.scrollToNote(note);
      highlight(note.id);
      closePanel();
    },
    [highlight, closePanel],
  );

  const navigateFindTarget = useCallback((target: DiffSearchMatchTarget) => {
    if (target.kind === 'commit-log-line') {
      document.getElementById(target.itemId)?.scrollIntoView({ block: 'center', behavior: 'smooth' });
      return;
    }
    codeViewRef.current?.scrollToFindTarget(target);
  }, []);

  const handleFindQueryChange = useCallback((query: string) => {
    find.setQuery(query);
    const nextProjection = query.length > 0
      ? buildDiffSearchProjection(committedDiff, uncommittedDiff, query, commitLog)
      : { sources: [], matches: [] };
    const target = nextProjection.matches[0]?.target;
    if (target) navigateFindTarget(target);
  }, [find, commitLog, committedDiff, navigateFindTarget, uncommittedDiff]);

  const handleFindNext = useCallback(() => {
    const nextIndex = findProjection.matches.length === 0
      ? -1
      : activeFindIndex < 0
        ? 0
        : (activeFindIndex + 1) % findProjection.matches.length;
    find.setActiveIndex(nextIndex);
    const target = nextIndex >= 0 ? findProjection.matches[nextIndex]?.target : null;
    if (target) navigateFindTarget(target);
  }, [activeFindIndex, find, findProjection.matches, navigateFindTarget]);

  const handleFindPrevious = useCallback(() => {
    const nextIndex = findProjection.matches.length === 0
      ? -1
      : activeFindIndex < 0
        ? Math.max(findProjection.matches.length - 1, 0)
        : (activeFindIndex - 1 + findProjection.matches.length) % findProjection.matches.length;
    find.setActiveIndex(nextIndex);
    const target = nextIndex >= 0 ? findProjection.matches[nextIndex]?.target : null;
    if (target) navigateFindTarget(target);
  }, [activeFindIndex, find, findProjection.matches, navigateFindTarget]);

  if (!open) return null;

  const empty = !commitLog.trim() && !committedDiff.trim() && !uncommittedDiff.trim();

  return (
    <ViewerShell
      closeOnEscape={!find.isOpen}
      onInnerEscape={closeFind}
      mode={inline ? 'inline' : takeover ? 'takeover' : 'overlay'}
      ariaLabel="Worktree diff"
      title={
        <span>
          Diff vs <code>{comparator}</code>
        </span>
      }
      headerExtras={
        <>
          <button
            className="viewer-shell-btn"
            type="button"
            ref={findButtonRef}
            onClick={openFind}
            aria-label="Find in diff"
            title="Find in diff"
          >
            Find
          </button>
          <button
            className="viewer-shell-btn"
            onClick={toggleDiffStyle}
            aria-label={diffStyle === 'unified' ? 'Switch to split view' : 'Switch to unified view'}
            title={diffStyle === 'unified' ? 'Split view' : 'Unified view'}
          >
            {diffStyle === 'unified' ? <Columns2 size={18} /> : <Rows3 size={18} />}
          </button>
        </>
      }
      banner={find.isOpen ? (
        <FindBar
          query={find.query}
          activeIndex={activeFindIndex}
          matchCount={findProjection.matches.length}
          focusVersion={find.focusVersion}
          onQueryChange={handleFindQueryChange}
          onNext={handleFindNext}
          onPrevious={handleFindPrevious}
          onClose={closeFind}
          autoFocus
        />
      ) : undefined}
      noteCount={diffNotes.length}
      onToggleNotes={notes.togglePanel}
      onSend={notes.send}
      onClose={onClose}
      bodyScroll="children"
      panel={
        notes.showPanel ? (
          // Panel scope = THIS viewer's notes. Cross-viewer notes
          // (file-anchored) live in the same global pile but only surface in
          // their own viewer's panel — Send All still drops the entire pile.
          <NotesPanel
            notes={diffNotes}
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
            anchorLabel={anchorDialogLabel(notes.annotating)}
            lineContent={notes.annotating.kind === 'line' ? notes.annotating.lineContent : ''}
            onSubmit={notes.submitNote}
            onCancel={notes.cancelAnnotate}
          />
        ) : null
      }
    >
      <div className="diff-viewer-body">
        {empty ? (
          <div className="diff-viewer-empty">
            No changes vs <code>{comparator}</code>.
          </div>
        ) : (
          <>
            {commitLog.trim() && (
              <CommitLogSection
                commitLog={commitLog}
                matches={findProjection.matches}
                activeMatchIndex={activeFindIndex}
                findOpen={find.isOpen}
              />
            )}
            <DiffSummaryBar
              comparator={comparator}
              committedDiff={committedDiff}
              committedTruncatedKib={committedTruncatedKib}
              committedSaturated={committedSaturated}
              uncommittedDiff={uncommittedDiff}
              uncommittedTruncatedKib={uncommittedTruncatedKib}
              uncommittedSaturated={uncommittedSaturated}
            />
            <PhoenixDiffCodeView
              ref={codeViewRef}
              committedDiff={committedDiff}
              uncommittedDiff={uncommittedDiff}
              diffStyle={diffStyle}
              findMatches={findMatchTargets}
              activeFindMatch={activeFindMatchTarget}
              notes={diffNotes}
              highlightedNoteId={notes.highlightedNoteId}
              onAnnotateLine={notes.startAnnotateLine}
              onAnnotateFile={notes.startAnnotateFile}
            />
          </>
        )}
      </div>
    </ViewerShell>
  );
}

function CommitLogSection({
  commitLog,
  matches,
  activeMatchIndex,
  findOpen,
}: {
  commitLog: string;
  matches: readonly { target: DiffSearchMatchTarget; start: number; end: number }[];
  activeMatchIndex: number;
  findOpen: boolean;
}) {
  const matchesByLine = new Map<number, Array<{ start: number; end: number; occurrenceIndex: number }>>();
  matches.forEach((match, occurrenceIndex) => {
    if (match.target.kind !== 'commit-log-line') return;
    const lineNumber = Number.parseInt(match.target.itemId.replace('commit-log:', ''), 10);
    if (Number.isNaN(lineNumber)) return;
    const lineMatches = matchesByLine.get(lineNumber) ?? [];
    lineMatches.push({ start: match.start, end: match.end, occurrenceIndex });
    matchesByLine.set(lineNumber, lineMatches);
  });

  return (
    <section className="diff-section">
      <h3 className="diff-section-title">Commits</h3>
      <div className="diff-pre diff-pre-log">
        {commitLog.split('\n').map((line, i) => {
          const lineMatches = findOpen ? matchesByLine.get(i) ?? [] : [];
          const isActive = lineMatches.some((match) => match.occurrenceIndex === activeMatchIndex);
          const hasMatch = lineMatches.length > 0;
          return (
            <div
              id={`commit-log:${i}`}
              key={i}
              className={[
                'diff-line',
                hasMatch && 'viewer-find-row-match',
                isActive && 'viewer-find-row-match--active',
              ].filter(Boolean).join(' ')}
            >
              {lineMatches.length === 0 ? (line || ' ') : renderCommitLogFindFragments(line, lineMatches, activeMatchIndex)}
            </div>
          );
        })}
      </div>
    </section>
  );
}

function renderCommitLogFindFragments(
  text: string,
  matches: readonly { start: number; end: number; occurrenceIndex: number }[],
  activeOccurrence: number,
): React.ReactNode[] {
  if (matches.length === 0) return [text || ' '];
  const fragments: React.ReactNode[] = [];
  let cursor = 0;
  matches.forEach((match) => {
    const start = Math.max(match.start, cursor);
    const end = Math.max(match.end, start);
    if (start > cursor) fragments.push(text.slice(cursor, start));
    if (end > start) {
      fragments.push(
        <mark
          key={`${match.start}-${match.end}-${match.occurrenceIndex}`}
          className={match.occurrenceIndex === activeOccurrence ? 'viewer-find-match viewer-find-match--active' : 'viewer-find-match'}
          data-find-occurrence={match.occurrenceIndex}
        >
          {text.slice(start, end)}
        </mark>,
      );
    }
    cursor = Math.max(cursor, end);
  });
  if (cursor < text.length) fragments.push(text.slice(cursor));
  return fragments.length > 0 ? fragments : [' '];
}

interface DiffSummaryBarProps {
  comparator: string;
  committedDiff: string;
  committedTruncatedKib?: number | undefined;
  committedSaturated?: boolean | undefined;
  uncommittedDiff: string;
  uncommittedTruncatedKib?: number | undefined;
  uncommittedSaturated?: boolean | undefined;
}

/** Section labels + truncation indicators above the diff surface. The diff
 *  itself is a single virtualized CodeView; each file header carries its own
 *  committed/uncommitted badge, so this strip provides the comparator context
 *  and the per-section truncation state. */
function DiffSummaryBar({
  comparator,
  committedDiff,
  committedTruncatedKib,
  committedSaturated,
  uncommittedDiff,
  uncommittedTruncatedKib,
  uncommittedSaturated,
}: DiffSummaryBarProps) {
  return (
    <div className="diff-summary-bar">
      {committedDiff.trim() && (
        <SectionLabel
          label={`Committed changes (vs ${comparator})`}
          section="committed"
          truncatedKib={committedTruncatedKib}
          saturated={committedSaturated}
        />
      )}
      {uncommittedDiff.trim() && (
        <SectionLabel
          label="Uncommitted changes"
          section="uncommitted"
          truncatedKib={uncommittedTruncatedKib}
          saturated={uncommittedSaturated}
        />
      )}
    </div>
  );
}

function SectionLabel({
  label,
  section,
  truncatedKib,
  saturated,
}: {
  label: string;
  section: DiffSection;
  truncatedKib?: number | undefined;
  saturated?: boolean | undefined;
}) {
  return (
    <div className={`diff-summary-section diff-summary-section--${section}`}>
      <span className={`phoenix-diff-section-badge phoenix-diff-section-badge--${section}`}>
        {section}
      </span>
      <span className="diff-summary-label">{label}</span>
      {truncatedKib !== undefined && (
        <span className="diff-section-truncated">
          (truncated; {saturated ? '≥' : ''}
          {truncatedKib} KiB total)
        </span>
      )}
    </div>
  );
}

function anchorDialogLabel(t: AnnotateTarget): string {
  if (t.kind === 'file') return `${t.filePath} (file-level)`;
  if (t.newLine !== undefined) return `${t.filePath}:${t.newLine}`;
  if (t.oldLine !== undefined) return `${t.filePath}:-${t.oldLine}`;
  return t.filePath;
}
