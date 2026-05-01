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
import type {
  DiffSection,
  NoteAnchor,
  ReviewNote,
} from '../../contexts/ReviewNotesContext';
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
  /** Render inline (no overlay) for desktop split-pane mode (08654). */
  inline?: boolean;
}

type AnnotateTarget =
  | { kind: 'line'; section: DiffSection; segment: DiffSegment; line: DiffLine }
  | { kind: 'file'; section: DiffSection; segment: DiffSegment; diffPos: number };

interface SectionDef {
  /** Header rendered above this section's diff. */
  title: string;
  /** Section discriminator — also flows onto note anchors so the
   *  per-section diffPos namespaces don't collide (a note at position 5
   *  in committed must look up a different ref/highlight than a note at
   *  position 5 in uncommitted). */
  id: DiffSection;
  body: string;
  truncatedKib?: number | undefined;
  /** When true, `truncatedKib` is a lower bound — render with "≥". */
  saturated?: boolean | undefined;
}

/** Compose a unique key from the section discriminator + the per-section
 *  diff position. Used by `lineRefs`, `noteSet`, and the highlight
 *  state so committed/uncommitted positions never collide. */
function diffKey(section: DiffSection, diffPos: number): string {
  return `${section}:${diffPos}`;
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
  const reviewNotes = useReviewNotes();

  const [annotating, setAnnotating] = useState<AnnotateTarget | null>(null);
  const [showPanel, setShowPanel] = useState(false);
  const [highlightedKey, setHighlightedKey] = useState<string | null>(null);
  // Keyed by `${section}:${diffPos}` so committed and uncommitted
  // positions occupy disjoint namespaces.
  const lineRefs = useRef<Map<string, HTMLElement>>(new Map());

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
        saturated: committedSaturated,
      },
      {
        id: 'uncommitted',
        title: 'Uncommitted changes',
        body: uncommittedDiff,
        truncatedKib: uncommittedTruncatedKib,
        saturated: uncommittedSaturated,
      },
    ],
    [
      comparator,
      committedDiff,
      committedTruncatedKib,
      committedSaturated,
      uncommittedDiff,
      uncommittedTruncatedKib,
      uncommittedSaturated,
    ],
  );

  // Clear highlight after animation
  useEffect(() => {
    if (highlightedKey !== null) {
      const timer = setTimeout(() => setHighlightedKey(null), 2000);
      return () => clearTimeout(timer);
    }
    return undefined;
  }, [highlightedKey]);

  const diffNotes = useMemo(() => reviewNotes.notesForDiff(), [reviewNotes]);

  const handleSubmitNote = useCallback(
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
    const key = diffKey(note.anchor.section, note.anchor.diffPos);
    const el = lineRefs.current.get(key);
    if (el) {
      el.scrollIntoView({ behavior: 'smooth', block: 'center' });
      setHighlightedKey(key);
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
      noteCount={diffNotes.length}
      onToggleNotes={() => setShowPanel((v) => !v)}
      onSend={handleSend}
      onClose={onClose}
      panel={
        showPanel ? (
          // Panel scope = THIS viewer's notes. Cross-viewer notes
          // (file-anchored) live in the same global pile but only
          // surface in their own viewer's panel — Send All still
          // drops the entire pile so the user doesn't lose them.
          <NotesPanel
            notes={diffNotes}
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
                    setAnnotating({ kind: 'line', section: s.id, segment, line })
                  }
                  onAnnotateFile={(segment, diffPos) =>
                    setAnnotating({ kind: 'file', section: s.id, segment, diffPos })
                  }
                  highlightedKey={highlightedKey}
                  // Section-scoped: filter diff notes to only those
                  // anchored in THIS section before computing the
                  // "has note" set, so the per-line indicator dots
                  // don't bleed across sections.
                  noteKeys={diffNotes.flatMap((n) => {
                    if (n.anchor.kind === 'diff' || n.anchor.kind === 'diff-file') {
                      if (n.anchor.section === s.id) {
                        return [diffKey(s.id, n.anchor.diffPos)];
                      }
                    }
                    return [];
                  })}
                  registerLineRef={(diffPos, el) => {
                    const key = diffKey(s.id, diffPos);
                    if (el) lineRefs.current.set(key, el);
                    else lineRefs.current.delete(key);
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
  /** Composite key (`${section}:${diffPos}`) of the currently
   *  highlighted line, or null. Section-scoped so committed/uncommitted
   *  positions don't collide. */
  highlightedKey: string | null;
  /** Composite keys of lines that have a note attached, scoped to
   *  THIS section. */
  noteKeys: string[];
  registerLineRef: (diffPos: number, el: HTMLElement | null) => void;
}

function DiffSection({
  section,
  onAnnotateLine,
  onAnnotateFile,
  highlightedKey,
  noteKeys,
  registerLineRef,
}: DiffSectionProps) {
  const segments = useMemo(() => parseUnifiedDiff(section.body), [section.body]);
  const noteSet = useMemo(() => new Set(noteKeys), [noteKeys]);

  return (
    <section className="diff-section">
      <h3 className="diff-section-title">
        {section.title}
        {section.truncatedKib !== undefined && (
          <span className="diff-section-truncated">
            (truncated; {section.saturated ? '≥' : ''}{section.truncatedKib} KiB total)
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
            const key = diffKey(section.id, line.diffPos);
            return (
              <DiffLineRow
                key={`${segIdx}-${line.diffPos}`}
                segment={seg}
                line={line}
                isFileHeader={isFileHeader}
                onAnnotateLine={onAnnotateLine}
                onAnnotateFile={onAnnotateFile}
                highlighted={highlightedKey === key}
                hasNote={noteSet.has(key)}
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
