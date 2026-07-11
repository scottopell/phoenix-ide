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

  const [diffStyle, setDiffStyle] = useState<DiffStyle>(initialDiffStyle);
  const find = useViewerFind({ text: '' });
  const findSearchActive = find.isOpen || find.query.length > 0;
  const findProjection = useMemo(
    () => (findSearchActive ? buildDiffSearchProjection(committedDiff, uncommittedDiff, find.query) : { sources: [], matches: [] }),
    [committedDiff, uncommittedDiff, find.query, findSearchActive],
  );
  useViewerFindKeyboardShortcut({ scopeId: 'diff-viewer', onOpen: find.open });

  const activeFindMatchTarget = find.isOpen && find.activeIndex >= 0 ? findProjection.matches[find.activeIndex]?.target ?? null : null;
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

  const handleFindQueryChange = useCallback((query: string) => {
    find.setQuery(query);
    const nextProjection = buildDiffSearchProjection(committedDiff, uncommittedDiff, query);
    const target = nextProjection.matches[0]?.target;
    if (target) codeViewRef.current?.scrollToFindTarget(target);
  }, [find, committedDiff, uncommittedDiff]);

  const handleFindNext = useCallback(() => {
    find.nextMatch();
    const nextIndex = find.matchCount === 0
      ? -1
      : find.activeIndex < 0
        ? 0
        : (find.activeIndex + 1) % find.matchCount;
    const target = nextIndex >= 0 ? findProjection.matches[nextIndex]?.target : null;
    if (target) codeViewRef.current?.scrollToFindTarget(target);
  }, [find, findProjection.matches]);

  const handleFindPrevious = useCallback(() => {
    find.previousMatch();
    const nextIndex = find.matchCount === 0
      ? -1
      : find.activeIndex < 0
        ? Math.max(find.matchCount - 1, 0)
        : (find.activeIndex - 1 + find.matchCount) % find.matchCount;
    const target = nextIndex >= 0 ? findProjection.matches[nextIndex]?.target : null;
    if (target) codeViewRef.current?.scrollToFindTarget(target);
  }, [find, findProjection.matches]);

  if (!open) return null;

  const empty = !commitLog.trim() && !committedDiff.trim() && !uncommittedDiff.trim();

  return (
    <ViewerShell
      closeOnEscape={!find.isOpen}
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
            onClick={find.open}
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
          activeIndex={find.activeIndex}
          matchCount={find.matchCount}
          onQueryChange={handleFindQueryChange}
          onNext={handleFindNext}
          onPrevious={handleFindPrevious}
          onClose={find.close}
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
            {commitLog.trim() && <CommitLogSection commitLog={commitLog} />}
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

function CommitLogSection({ commitLog }: { commitLog: string }) {
  return (
    <section className="diff-section">
      <h3 className="diff-section-title">Commits</h3>
      <div className="diff-pre diff-pre-log">
        {commitLog.split('\n').map((line, i) => (
          <div key={i} className="diff-line">
            {line || ' '}
          </div>
        ))}
      </div>
    </section>
  );
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
