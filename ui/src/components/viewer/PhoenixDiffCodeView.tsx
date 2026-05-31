/**
 * PhoenixDiffCodeView — the single boundary between Phoenix and `@pierre/diffs`.
 *
 * Pierre types stay behind this wrapper: it accepts raw committed/uncommitted
 * diff text plus Phoenix review notes, parses each section into Pierre
 * `CodeView` items (committed items first, then uncommitted — section identity
 * is structural in the item id), maps notes to/from Pierre line annotations,
 * and exposes a typed `scrollToNote` for NotesPanel jumps. No annotation, note
 * indicator, line identity, or jump path ever scrapes Pierre's DOM.
 *
 * Note affordances:
 *   - line notes: gutter `+` on the hovered line (pointer), click-to-annotate
 *     (mouse/pen), and a 500ms long-press (touch — cancels on movement).
 *   - file-level notes: a `+` button in the file header metadata slot.
 * Both flow up through `onAnnotateLine` / `onAnnotateFile` to the shared
 * `useDiffReviewNotes` lifecycle.
 */

import { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useRef } from 'react';
import { MessageSquarePlus } from 'lucide-react';
import { CodeView } from '@pierre/diffs/react';
import type { CodeViewHandle, CodeViewItem } from '@pierre/diffs/react';
import type { AnnotationSide, CodeViewOptions, FileDiffMetadata } from '@pierre/diffs';
import type { DiffSection, ReviewNote } from '../../contexts/ReviewNotesContext';
import { useTheme } from '../../hooks/useTheme';
import type { LineAnnotateTarget } from './useDiffReviewNotes';
import {
  annotationsForFile,
  buildSectionItems,
  fileNotesFor,
  itemRenderSignature,
  lineTextAt,
  scrollTargetForNote,
  sectionFromItemId,
  type PhoenixDiffAnnotationMeta,
  type PhoenixDiffItem,
} from './pierreDiffMapping';

export interface PhoenixDiffCodeViewProps {
  committedDiff: string;
  uncommittedDiff: string;
  diffStyle: 'unified' | 'split';
  /** Diff-scoped review notes (committed + uncommitted), already filtered to
   *  diff anchors by the caller. */
  notes: readonly ReviewNote[];
  /** Note id to flash after a jump, or null. */
  highlightedNoteId: string | null;
  onAnnotateLine: (target: Omit<LineAnnotateTarget, 'kind'>) => void;
  onAnnotateFile: (section: DiffSection, filePath: string) => void;
}

export interface PhoenixDiffCodeViewHandle {
  /** Scroll to (and the caller then flashes) the note's anchored line/file via
   *  Pierre's typed scroll target — never DOM lookup. */
  scrollToNote: (note: ReviewNote) => void;
}

type Meta = PhoenixDiffAnnotationMeta;

export const PhoenixDiffCodeView = forwardRef<PhoenixDiffCodeViewHandle, PhoenixDiffCodeViewProps>(
  function PhoenixDiffCodeView(
    { committedDiff, uncommittedDiff, diffStyle, notes, highlightedNoteId, onAnnotateLine, onAnnotateFile },
    ref,
  ) {
    const { theme } = useTheme();
    const codeViewRef = useRef<CodeViewHandle<Meta>>(null);
    // CodeView root element — long-press pointer listeners attach here (line
    // pointer events are composed and bubble out of Pierre's shadow DOM).
    const containerRef = useRef<HTMLDivElement>(null);
    // The diff line the pointer is currently over, tracked via Pierre's typed
    // onLineEnter/onLineLeave (no DOM scraping). The long-press timer reads this
    // at fire time to know which line to annotate.
    type HoveredLine = { section: DiffSection; fileDiff: FileDiffMetadata; side: AnnotationSide; lineNumber: number };
    const hoveredLine = useRef<HoveredLine | null>(null);
    // Monotonic per-item version. Pierre's controlled reconciler reuses a record
    // with the same id unless its `version` changes, so we bump the version
    // whenever an item's render signature (file content + its notes + flash)
    // changes. Keyed by item id; survives reparses.
    const itemVersions = useRef<Map<string, { sig: string; version: number }>>(new Map());

    // Parse is the expensive step — memoize on raw text only, independent of
    // note churn. Annotation attachment (cheap) re-runs when notes change.
    const committed = useMemo(() => buildSectionItems('committed', committedDiff), [committedDiff]);
    const uncommitted = useMemo(() => buildSectionItems('uncommitted', uncommittedDiff), [uncommittedDiff]);

    const items = useMemo<PhoenixDiffItem[]>(() => {
      const vmap = itemVersions.current;
      const attach = (built: PhoenixDiffItem[]): PhoenixDiffItem[] =>
        built.map((it) => {
          const section = sectionFromItemId(it.id);
          if (!section) return it;
          const anns = annotationsForFile(notes, section, it.fileDiff.name);
          // Bump the controlled-item version whenever this item's rendered
          // content changes, so Pierre reconciles instead of keeping a stale
          // annotation/flash/header-count record.
          const sig = itemRenderSignature(it.fileDiff, notes, section, highlightedNoteId);
          const prev = vmap.get(it.id);
          const version = prev && prev.sig === sig ? prev.version : (prev?.version ?? 0) + 1;
          if (!prev || prev.sig !== sig) vmap.set(it.id, { sig, version });
          return anns.length > 0 ? { ...it, annotations: anns, version } : { ...it, version };
        });
      return [...attach(committed.items), ...attach(uncommitted.items)];
    }, [committed.items, uncommitted.items, notes, highlightedNoteId]);

    const annotateLine = useCallback(
      (section: DiffSection, fileDiff: FileDiffMetadata, side: AnnotationSide, lineNumber: number) => {
        const lineContent = lineTextAt(fileDiff, side, lineNumber) ?? '';
        if (side === 'additions') {
          onAnnotateLine({ section, filePath: fileDiff.name, newLine: lineNumber, lineContent });
        } else {
          onAnnotateLine({ section, filePath: fileDiff.name, oldLine: lineNumber, lineContent });
        }
      },
      [onAnnotateLine],
    );

    const options = useMemo<CodeViewOptions<Meta>>(
      () => ({
        theme: { dark: 'pierre-dark', light: 'pierre-light' },
        themeType: theme,
        diffStyle,
        stickyHeaders: true,
        enableGutterUtility: true,
        // All items are diff items; the file overload never fires at runtime.
        // The option callback is an overload intersection (file + diff); we
        // implement the diff case and narrow on context.type, casting through
        // `unknown` to satisfy the intersection without re-stating both shapes.
        onLineClick: ((
          props: { annotationSide: AnnotationSide; lineNumber: number; event?: { pointerType?: string } },
          context: { type: 'diff' | 'file'; item: PhoenixDiffItem },
        ) => {
          if (context.type !== 'diff') return;
          // Touch annotates via long-press, not tap, so a tap doesn't eagerly
          // open the dialog (the long-press handler owns the touch path).
          if (props.event?.pointerType === 'touch') return;
          const section = sectionFromItemId(context.item.id);
          if (!section) return;
          annotateLine(section, context.item.fileDiff, props.annotationSide, props.lineNumber);
        }) as unknown as NonNullable<CodeViewOptions<Meta>['onLineClick']>,
        // Track the hovered/pressed line for the long-press handler.
        onLineEnter: ((
          props: { annotationSide: AnnotationSide; lineNumber: number },
          context: { type: 'diff' | 'file'; item: PhoenixDiffItem },
        ) => {
          if (context.type !== 'diff') return;
          const section = sectionFromItemId(context.item.id);
          if (!section) return;
          hoveredLine.current = {
            section,
            fileDiff: context.item.fileDiff,
            side: props.annotationSide,
            lineNumber: props.lineNumber,
          };
        }) as unknown as NonNullable<CodeViewOptions<Meta>['onLineEnter']>,
        onLineLeave: (() => {
          hoveredLine.current = null;
        }) as unknown as NonNullable<CodeViewOptions<Meta>['onLineLeave']>,
      }),
      [theme, diffStyle, annotateLine],
    );

    // Touch long-press → annotate the line under the finger. Pierre owns line
    // pointer handling, so we listen on the (composed) container and read the
    // typed hovered line; a 500ms hold with no movement opens the dialog.
    useEffect(() => {
      const el = containerRef.current;
      if (!el) return undefined;
      const MOVE_CANCEL_PX = 10;
      const HOLD_MS = 500;
      let timer: ReturnType<typeof setTimeout> | undefined;
      let startX = 0;
      let startY = 0;
      const clear = () => {
        if (timer) clearTimeout(timer);
        timer = undefined;
      };
      const onDown = (e: PointerEvent) => {
        if (e.pointerType !== 'touch') return;
        startX = e.clientX;
        startY = e.clientY;
        clear();
        timer = setTimeout(() => {
          const l = hoveredLine.current;
          if (l) annotateLine(l.section, l.fileDiff, l.side, l.lineNumber);
        }, HOLD_MS);
      };
      const onMove = (e: PointerEvent) => {
        if (timer && (Math.abs(e.clientX - startX) > MOVE_CANCEL_PX || Math.abs(e.clientY - startY) > MOVE_CANCEL_PX)) {
          clear();
        }
      };
      el.addEventListener('pointerdown', onDown);
      el.addEventListener('pointermove', onMove);
      el.addEventListener('pointerup', clear);
      el.addEventListener('pointercancel', clear);
      return () => {
        clear();
        el.removeEventListener('pointerdown', onDown);
        el.removeEventListener('pointermove', onMove);
        el.removeEventListener('pointerup', clear);
        el.removeEventListener('pointercancel', clear);
      };
    }, [annotateLine]);

    const renderAnnotation = useCallback(
      (annotation: { metadata?: Meta }) => {
        const meta = annotation.metadata;
        if (!meta) return null;
        const note = notes.find((n) => n.id === meta.noteId);
        if (!note) return null;
        const flash = meta.noteId === highlightedNoteId;
        return (
          <div
            className={`phoenix-diff-note${flash ? ' phoenix-diff-note--flash' : ''}`}
            role="note"
          >
            <span className="phoenix-diff-note-body">{note.body}</span>
          </div>
        );
      },
      [notes, highlightedNoteId],
    );

    const renderHeaderPrefix = useCallback((item: CodeViewItem<Meta>) => {
      const section = sectionFromItemId(item.id);
      if (!section) return null;
      return (
        <span className={`phoenix-diff-section-badge phoenix-diff-section-badge--${section}`}>
          {section}
        </span>
      );
    }, []);

    const renderHeaderMetadata = useCallback(
      (item: CodeViewItem<Meta>) => {
        if (item.type !== 'diff') return null;
        const section = sectionFromItemId(item.id);
        if (!section) return null;
        const filePath = item.fileDiff.name;
        const count = fileNotesFor(notes, section, filePath).length;
        return (
          <button
            type="button"
            className="phoenix-diff-file-note-btn"
            onClick={() => onAnnotateFile(section, filePath)}
            aria-label={`Add file-level note to ${filePath}`}
            title="Add file-level note"
          >
            <MessageSquarePlus size={14} />
            {count > 0 && <span className="phoenix-diff-file-note-count">{count}</span>}
          </button>
        );
      },
      [notes, onAnnotateFile],
    );

    const renderGutterUtility = useCallback(
      (getHoveredLine: () => { lineNumber: number; side: AnnotationSide } | undefined, item: CodeViewItem<Meta>) => {
        if (item.type !== 'diff') return null;
        const fileDiff = item.fileDiff;
        const id = item.id;
        return (
          <button
            type="button"
            className="phoenix-diff-add-note"
            aria-label="Add note to line"
            title="Add note"
            onClick={(e) => {
              e.stopPropagation();
              const hovered = getHoveredLine();
              if (!hovered) return;
              const section = sectionFromItemId(id);
              if (!section) return;
              annotateLine(section, fileDiff, hovered.side, hovered.lineNumber);
            }}
          >
            <MessageSquarePlus size={12} />
          </button>
        );
      },
      [annotateLine],
    );

    useImperativeHandle(
      ref,
      () => ({
        scrollToNote: (note: ReviewNote) => {
          const target = scrollTargetForNote(note);
          const cv = codeViewRef.current;
          if (!target || !cv) return;
          if (target.line) {
            cv.scrollTo({
              type: 'line',
              id: target.id,
              lineNumber: target.line.lineNumber,
              side: target.line.side,
              align: 'center',
              behavior: 'smooth',
            });
          } else {
            cv.scrollTo({ type: 'item', id: target.id, align: 'start', behavior: 'smooth' });
          }
        },
      }),
      [],
    );

    const parseError = committed.error ?? uncommitted.error;

    return (
      <div className="phoenix-diff-codeview-wrap">
        {committed.error && (
          <div className="diff-section-error" role="alert">
            Committed diff could not be parsed: {committed.error}
          </div>
        )}
        {uncommitted.error && (
          <div className="diff-section-error" role="alert">
            Uncommitted diff could not be parsed: {uncommitted.error}
          </div>
        )}
        {items.length > 0 ? (
          <CodeView<Meta>
            ref={codeViewRef}
            containerRef={containerRef}
            items={items}
            options={options}
            className="phoenix-diff-codeview"
            renderAnnotation={renderAnnotation}
            renderHeaderPrefix={renderHeaderPrefix}
            renderHeaderMetadata={renderHeaderMetadata}
            renderGutterUtility={renderGutterUtility}
          />
        ) : (
          !parseError && <div className="diff-viewer-empty">No file changes to display.</div>
        )}
      </div>
    );
  },
);
