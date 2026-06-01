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

import { useCallback, useRef, useState } from 'react';
import { Columns2, Rows3 } from 'lucide-react';
import type { DiffSection, ReviewNote } from '../../contexts/ReviewNotesContext';
import { useRegisterFocusScope } from '../../hooks/useFocusScope';
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
  inline,
}: DiffViewProps) {
  useRegisterFocusScope('diff-viewer');
  const notes = useDiffReviewNotes(onSendNotes);
  const codeViewRef = useRef<PhoenixDiffCodeViewHandle>(null);

  const [diffStyle, setDiffStyle] = useState<DiffStyle>(initialDiffStyle);
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

  if (!open) return null;

  const empty = !commitLog.trim() && !committedDiff.trim() && !uncommittedDiff.trim();

  return (
    <ViewerShell
      mode={inline ? 'inline' : 'overlay'}
      ariaLabel="Worktree diff"
      title={
        <span>
          Diff vs <code>{comparator}</code>
        </span>
      }
      headerExtras={
        <button
          className="viewer-shell-btn"
          onClick={toggleDiffStyle}
          aria-label={diffStyle === 'unified' ? 'Switch to split view' : 'Switch to unified view'}
          title={diffStyle === 'unified' ? 'Split view' : 'Unified view'}
        >
          {diffStyle === 'unified' ? <Columns2 size={18} /> : <Rows3 size={18} />}
        </button>
      }
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
