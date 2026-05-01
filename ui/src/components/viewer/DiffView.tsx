/**
 * DiffView — multi-file unified-diff viewer with review-notes
 * integration. Built on the shared `viewer/` primitives.
 *
 * Long-press on any add/del/context line opens the AnnotationDialog
 * to add a note anchored to (filePath, newLine, diffPos). Long-press
 * on a `diff --git` header creates a file-level note. Notes share the
 * conversation-scoped pile with FileView; Send drops the entire pile.
 */

import { useEffect, useMemo, useState, useCallback, useRef } from 'react';
import { MessageSquarePlus } from 'lucide-react';
import { useReviewNotes } from '../../contexts/ReviewNotesContext';
import type { NoteAnchor, ReviewNote } from '../../contexts/ReviewNotesContext';
import { useLongPress } from '../../hooks/useLongPress';
import { useRegisterFocusScope } from '../../hooks/useFocusScope';
import { ViewerShell } from './ViewerShell';
import { NotesPanel } from './NotesPanel';
import { AnnotationDialog } from './AnnotationDialog';
import { formatNotesForSend } from './formatNotes';
import { parseUnifiedDiff } from './diffParse';
import type { DiffLine, DiffSegment } from './diffParse';

export interface DiffViewProps {
  open: boolean;
  comparator: string;
  commitLog: string;
  committedDiff: string;
  committedTruncatedKib?: number | undefined;
  uncommittedDiff: string;
  uncommittedTruncatedKib?: number | undefined;
  onClose: () => void;
  /** Drop the formatted review-notes pile into the chat input. Same
   *  signature as ProseReader's onSendNotes. */
  onSendNotes: (notes: string) => void;
  /** Render inline (no overlay) for desktop split-pane mode (08654). */
  inline?: boolean;
}

type AnnotateTarget =
  | { kind: 'line'; segment: DiffSegment; line: DiffLine }
  | { kind: 'file'; segment: DiffSegment; diffPos: number };

interface SectionDef {
  /** Header rendered above this section's diff. */
  title: string;
  /** Stable id for note `diffPos` namespace — combined with the
   *  position-in-text so notes from the committed diff don't collide
   *  with notes from the uncommitted diff. */
  id: 'committed' | 'uncommitted';
  body: string;
  truncatedKib?: number | undefined;
}

export function DiffView({
  open,
  comparator,
  commitLog,
  committedDiff,
  committedTruncatedKib,
  uncommittedDiff,
  uncommittedTruncatedKib,
  onClose,
  onSendNotes,
  inline,
}: DiffViewProps) {
  useRegisterFocusScope('diff-viewer');
  const reviewNotes = useReviewNotes();

  const [annotating, setAnnotating] = useState<AnnotateTarget | null>(null);
  const [showPanel, setShowPanel] = useState(false);
  const [highlightedDiffPos, setHighlightedDiffPos] = useState<number | null>(null);
  const lineRefs = useRef<Map<number, HTMLElement>>(new Map());

  // Two parsed sections — committed and uncommitted. Each segment-list
  // is parsed independently. We compose a stable diffPos namespace by
  // prefixing position with the section id, so notes anchor uniquely.
  const sections: SectionDef[] = useMemo(
    () => [
      {
        id: 'committed',
        title: `Committed changes (vs ${comparator})`,
        body: committedDiff,
        truncatedKib: committedTruncatedKib,
      },
      {
        id: 'uncommitted',
        title: 'Uncommitted changes',
        body: uncommittedDiff,
        truncatedKib: uncommittedTruncatedKib,
      },
    ],
    [comparator, committedDiff, committedTruncatedKib, uncommittedDiff, uncommittedTruncatedKib],
  );

  // Clear highlight after animation
  useEffect(() => {
    if (highlightedDiffPos !== null) {
      const timer = setTimeout(() => setHighlightedDiffPos(null), 2000);
      return () => clearTimeout(timer);
    }
    return undefined;
  }, [highlightedDiffPos]);

  const diffNotes = useMemo(() => reviewNotes.notesForDiff(), [reviewNotes]);
  const totalNotes = reviewNotes.notes.length;

  const handleSubmitNote = useCallback(
    (body: string) => {
      if (!annotating) return;
      let anchor: NoteAnchor;
      let lineContent: string;
      if (annotating.kind === 'line') {
        anchor = {
          kind: 'diff',
          filePath: annotating.segment.filePath,
          newLine: annotating.line.newLine,
          oldLine: annotating.line.oldLine,
          diffPos: annotating.line.diffPos,
        };
        lineContent = annotating.line.text;
      } else {
        anchor = {
          kind: 'diff-file',
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

  const handleSend = useCallback(() => {
    const formatted = formatNotesForSend(reviewNotes.notes);
    if (formatted) {
      onSendNotes(formatted);
      reviewNotes.clear();
      setShowPanel(false);
    }
  }, [reviewNotes, onSendNotes]);

  const handleJumpTo = useCallback((note: ReviewNote) => {
    if (note.anchor.kind !== 'diff' && note.anchor.kind !== 'diff-file') return;
    const el = lineRefs.current.get(note.anchor.diffPos);
    if (el) {
      el.scrollIntoView({ behavior: 'smooth', block: 'center' });
      setHighlightedDiffPos(note.anchor.diffPos);
    }
    setShowPanel(false);
  }, []);

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
      noteCount={totalNotes}
      onToggleNotes={() => setShowPanel((v) => !v)}
      onSend={handleSend}
      onClose={onClose}
      panel={
        showPanel ? (
          <NotesPanel
            notes={reviewNotes.notes}
            onJumpTo={handleJumpTo}
            onRemove={reviewNotes.removeNote}
            onClearAll={() => { reviewNotes.clear(); setShowPanel(false); }}
            onSend={handleSend}
            onClose={() => setShowPanel(false)}
          />
        ) : null
      }
      dialog={
        annotating ? (
          <AnnotationDialog
            anchorLabel={anchorDialogLabel(annotating)}
            lineContent={annotating.kind === 'line' ? annotating.line.text : ''}
            onSubmit={handleSubmitNote}
            onCancel={() => setAnnotating(null)}
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
              <CommitLogSection commitLog={commitLog} />
            )}
            {sections.map((s) =>
              s.body.trim() ? (
                <DiffSection
                  key={s.id}
                  section={s}
                  onAnnotateLine={(segment, line) =>
                    setAnnotating({ kind: 'line', segment, line })
                  }
                  onAnnotateFile={(segment, diffPos) =>
                    setAnnotating({ kind: 'file', segment, diffPos })
                  }
                  highlightedDiffPos={highlightedDiffPos}
                  noteDiffPositions={diffNotes.flatMap((n) =>
                    n.anchor.kind === 'diff' || n.anchor.kind === 'diff-file'
                      ? [n.anchor.diffPos]
                      : [],
                  )}
                  registerLineRef={(diffPos, el) => {
                    if (el) lineRefs.current.set(diffPos, el);
                    else lineRefs.current.delete(diffPos);
                  }}
                />
              ) : null,
            )}
          </>
        )}
      </div>
      {/* Shimmer to indicate loading would go here in a future iteration. */}
      <DiffViewLoadingShim />
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

interface DiffSectionProps {
  section: SectionDef;
  onAnnotateLine: (segment: DiffSegment, line: DiffLine) => void;
  onAnnotateFile: (segment: DiffSegment, diffPos: number) => void;
  highlightedDiffPos: number | null;
  noteDiffPositions: number[];
  registerLineRef: (diffPos: number, el: HTMLElement | null) => void;
}

function DiffSection({
  section,
  onAnnotateLine,
  onAnnotateFile,
  highlightedDiffPos,
  noteDiffPositions,
  registerLineRef,
}: DiffSectionProps) {
  const segments = useMemo(() => parseUnifiedDiff(section.body), [section.body]);
  const noteSet = useMemo(() => new Set(noteDiffPositions), [noteDiffPositions]);

  return (
    <section className="diff-section">
      <h3 className="diff-section-title">
        {section.title}
        {section.truncatedKib !== undefined && (
          <span className="diff-section-truncated">
            (truncated; {section.truncatedKib} KiB total)
          </span>
        )}
      </h3>
      <div className="diff-pre diff-pre-diff" role="region" aria-label={section.title}>
        {segments.map((seg, segIdx) =>
          seg.lines.map((line) => {
            // First line of each segment is the `diff --git` header — that
            // line is the file-level annotate target.
            const isFileHeader =
              line.kind === 'file-header' && line.text.startsWith('diff --git');
            return (
              <DiffLineRow
                key={`${segIdx}-${line.diffPos}`}
                segment={seg}
                line={line}
                isFileHeader={isFileHeader}
                onAnnotateLine={onAnnotateLine}
                onAnnotateFile={onAnnotateFile}
                highlighted={highlightedDiffPos === line.diffPos}
                hasNote={noteSet.has(line.diffPos)}
                registerRef={(el) => registerLineRef(line.diffPos, el)}
              />
            );
          }),
        )}
      </div>
    </section>
  );
}

interface DiffLineRowProps {
  segment: DiffSegment;
  line: DiffLine;
  isFileHeader: boolean;
  onAnnotateLine: (segment: DiffSegment, line: DiffLine) => void;
  onAnnotateFile: (segment: DiffSegment, diffPos: number) => void;
  highlighted: boolean;
  hasNote: boolean;
  registerRef: (el: HTMLElement | null) => void;
}

function DiffLineRow({
  segment,
  line,
  isFileHeader,
  onAnnotateLine,
  onAnnotateFile,
  highlighted,
  hasNote,
  registerRef,
}: DiffLineRowProps) {
  const annotatable =
    isFileHeader || line.kind === 'add' || line.kind === 'del' || line.kind === 'context';

  const lp = useLongPress<void>(() => {
    if (isFileHeader) onAnnotateFile(segment, line.diffPos);
    else onAnnotateLine(segment, line);
  });

  const cls = [
    'diff-line',
    `diff-${line.kind}`,
    annotatable && 'annotatable',
    highlighted && 'annotatable--highlighted',
    hasNote && 'diff-line--has-note',
  ]
    .filter(Boolean)
    .join(' ');

  if (!annotatable) {
    return (
      <div ref={registerRef} className={cls} data-diff-pos={line.diffPos}>
        {line.text || ' '}
      </div>
    );
  }

  return (
    <div
      ref={registerRef}
      className={cls}
      data-diff-pos={line.diffPos}
      onTouchStart={(e) => lp.start(e, undefined)}
      onTouchMove={lp.move}
      onTouchEnd={lp.end}
      onMouseDown={(e) => lp.start(e, undefined)}
      onMouseMove={lp.move}
      onMouseUp={lp.end}
      onMouseLeave={lp.end}
    >
      {line.text || ' '}
      <button
        className="annotatable__btn"
        onClick={(e) => {
          e.stopPropagation();
          if (isFileHeader) onAnnotateFile(segment, line.diffPos);
          else onAnnotateLine(segment, line);
        }}
        aria-label="Add note"
        title="Add note"
      >
        <MessageSquarePlus size={14} />
      </button>
    </div>
  );
}

function anchorDialogLabel(t: AnnotateTarget): string {
  if (t.kind === 'file') return `${t.segment.filePath} (file-level)`;
  const { line, segment } = t;
  if (line.newLine !== undefined) return `${segment.filePath}:${line.newLine}`;
  if (line.oldLine !== undefined) return `${segment.filePath}:-${line.oldLine}`;
  return `${segment.filePath} (diff position ${line.diffPos})`;
}

// Reserved for a future loading/error UI inside the shell. Currently
// DiffView receives pre-fetched diff text from its parent so there is
// nothing in-component to render here.
function DiffViewLoadingShim() {
  return null;
}
