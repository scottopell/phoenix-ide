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

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Columns2, Rows3 } from 'lucide-react';
import type { DiffSection, ReviewNote } from '../../contexts/ReviewNotesContext';
import { useRegisterFocusScope } from '../../hooks/useFocusScope';
import {
  FindBar,
  activeSessionMatchIndex,
  buildDiffSearchProjection,
  createSurfaceKey,
  projectionMatchesToSessionMatches,
  useFindSession,
  useViewerFindKeyboardShortcut,
  type DiffSearchMatchTarget,
  type FindSessionCommand,
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
  label?: string;
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
const DIFF_FIND_SURFACE_KEY = createSurfaceKey('diff-viewer');

type DiffFindFocusOrigin = HTMLElement | { readonly token: 'diff-find-button' };

function initialDiffStyle(): DiffStyle {
  const stored = localStorage.getItem(DIFF_STYLE_KEY);
  return stored === 'split' ? 'split' : 'unified';
}

export function DiffView({
  open,
  comparator,
  label,
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
  useRegisterFocusScope(open ? 'diff-viewer' : null);
  const notes = useDiffReviewNotes(onSendNotes);
  const codeViewRef = useRef<PhoenixDiffCodeViewHandle>(null);
  const findButtonRef = useRef<HTMLButtonElement>(null);
  const findPreviousFocusRef = useRef<HTMLElement | null>(null);

  const [diffStyle, setDiffStyle] = useState<DiffStyle>(initialDiffStyle);
  const restoreFocus = useCallback((focusOrigin: DiffFindFocusOrigin) => {
    if (focusOrigin instanceof HTMLElement) {
      queueMicrotask(() => focusOrigin.focus());
      return;
    }
    queueMicrotask(() => findButtonRef.current?.focus());
  }, []);

  const navigateFindTarget = useCallback((target: DiffSearchMatchTarget) => {
    if (target.kind === 'commit-log-line') {
      document.getElementById(target.itemId)?.scrollIntoView({ block: 'center', behavior: 'smooth' });
      return;
    }
    codeViewRef.current?.scrollToFindTarget(target);
  }, []);

  const handleFindCommands = useCallback((commands: readonly FindSessionCommand<DiffSearchMatchTarget, DiffFindFocusOrigin>[]) => {
    commands.forEach((command) => {
      switch (command.kind) {
        case 'focus-query':
          break;
        case 'restore-focus':
          restoreFocus(command.focusOrigin);
          break;
        case 'reveal-match':
          navigateFindTarget(command.target);
          break;
        case 'clear-decorations':
          break;
      }
    });
  }, [navigateFindTarget, restoreFocus]);
  const { state: findState, send: sendFind } = useFindSession<DiffSearchMatchTarget, DiffFindFocusOrigin>({
    onCommands: handleFindCommands,
  });

  const openFind = useCallback(() => {
    const focusOrigin = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : { token: 'diff-find-button' as const };
    findPreviousFocusRef.current = focusOrigin instanceof HTMLElement ? focusOrigin : null;
    sendFind({
      type: 'open',
      surface: {
        key: DIFF_FIND_SURFACE_KEY,
        query: '',
        matches: [],
        focusOrigin,
      },
    });
  }, [sendFind]);

  const closeFind = useCallback(() => {
    sendFind({ type: 'close' });
  }, [sendFind]);

  useEffect(() => {
    if (open) return;
    sendFind({ type: 'reset' });
    findPreviousFocusRef.current = null;
  }, [open, sendFind]);

  useViewerFindKeyboardShortcut({
    scopeId: 'diff-viewer',
    onOpen: openFind,
    dialogOpen: !open || notes.annotating !== null,
  });
  const findSession = findState.status === 'open' ? findState : null;
  const findOpen = findSession !== null;
  const findQuery = findSession?.query ?? '';
  const findProjection = useMemo(
    () => (findQuery.length > 0
      ? buildDiffSearchProjection(committedDiff, uncommittedDiff, findQuery, commitLog)
      : { sources: [], matches: [] }),
    [commitLog, committedDiff, findQuery, uncommittedDiff],
  );
  const sessionMatches = useMemo(
    () => projectionMatchesToSessionMatches(findProjection.matches, stableDiffMatchIds()),
    [findProjection.matches],
  );
  const activeFindIndex = findSession ? activeSessionMatchIndex(findSession.matches, findSession.activeMatchId) : -1;
  const activeFindMatchTarget = activeFindIndex >= 0 ? findSession?.matches[activeFindIndex]?.target ?? null : null;
  const findMatchTargets = useMemo(
    () => (findSession ? findSession.matches.map((match) => match.target) : []),
    [findSession],
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

  useEffect(() => {
    if (!findOpen || findQuery.length === 0) return;
    sendFind({ type: 'replace-results', matches: sessionMatches });
  }, [findOpen, findQuery, sendFind, sessionMatches]);

  const handleFindQueryChange = useCallback((query: string) => {
    sendFind({ type: 'set-query', query });
  }, [sendFind]);

  const handleFindNext = useCallback(() => {
    sendFind({ type: 'next' });
  }, [sendFind]);

  const handleFindPrevious = useCallback(() => {
    sendFind({ type: 'previous' });
  }, [sendFind]);

  if (!open) return null;

  const empty = !commitLog.trim() && !committedDiff.trim() && !uncommittedDiff.trim();

  return (
    <ViewerShell
      closeOnEscape={!findOpen}
      onInnerEscape={closeFind}
      mode={inline ? 'inline' : takeover ? 'takeover' : 'overlay'}
      ariaLabel={label ?? 'Worktree diff'}
      title={
        <span>
          {label ?? 'Diff'} vs <code>{comparator}</code>
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
      banner={findSession ? (
        <FindBar
          query={findSession.query}
          activeIndex={activeFindIndex}
          matchCount={findSession.matches.length}
          focusVersion={findSession.focusVersion}
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
                findOpen={findOpen}
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

function boundedDiffMatchHash(text: string): string {
  let hash = 0x811c9dc5;
  for (let index = 0; index < text.length; index += 1) {
    hash ^= text.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193);
  }
  return (hash >>> 0).toString(36);
}

function stableDiffMatchIds(): (match: {
  sourceId: string;
  sourceText: string;
  start: number;
  end: number;
  target: DiffSearchMatchTarget;
}) => string {
  const duplicateOccurrences = new Map<string, number>();
  return (match) => {
    const target = match.target;
    const semanticSignature = `${target.kind}:${target.itemId}:${target.side ?? ''}:${match.start}:${match.end}:${boundedDiffMatchHash(match.sourceText)}`;
    const duplicateOccurrence = duplicateOccurrences.get(semanticSignature) ?? 0;
    duplicateOccurrences.set(semanticSignature, duplicateOccurrence + 1);
    return `${semanticSignature}:${duplicateOccurrence}`;
  };
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

// eslint-disable-next-line react-refresh/only-export-components
export const __diffViewFindTestables = { stableDiffMatchIds };
