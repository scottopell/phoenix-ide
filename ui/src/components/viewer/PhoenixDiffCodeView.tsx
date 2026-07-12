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
import type { DiffSearchMatchTarget } from '../viewer-find';
import type { DiffSection, ReviewNote } from '../../contexts/ReviewNotesContext';
import { useTheme } from '../../hooks/useTheme';
import type { LineAnnotateTarget } from './useDiffReviewNotes';
import {
  annotationsForFile,
  buildSectionItems,
  fileNotesFor,
  itemRenderSignature,
  lineTextAt,
  resolveDiffAnchorLine,
  resolveTouchedLine,
  scrollTargetForNote,
  sectionFromItemId,
  type PhoenixDiffAnnotationMeta,
  type PhoenixDiffItem,
} from './pierreDiffMapping';

export interface PhoenixDiffCodeViewProps {
  committedDiff: string;
  uncommittedDiff: string;
  diffStyle: 'unified' | 'split';
  findMatches?: readonly DiffSearchMatchTarget[] | undefined;
  activeFindMatch?: DiffSearchMatchTarget | null | undefined;
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
  scrollToFindTarget: (target: DiffSearchMatchTarget) => void;
}

type Meta = PhoenixDiffAnnotationMeta;

export const PhoenixDiffCodeView = forwardRef<PhoenixDiffCodeViewHandle, PhoenixDiffCodeViewProps>(
  function PhoenixDiffCodeView(
    { committedDiff, uncommittedDiff, diffStyle, findMatches = [], activeFindMatch = null, notes, highlightedNoteId, onAnnotateLine, onAnnotateFile },
    ref,
  ) {
    const { theme } = useTheme();
    const codeViewRef = useRef<CodeViewHandle<Meta>>(null);
    // CodeView root element — long-press pointer listeners attach here (line
    // pointer events are composed and bubble out of Pierre's shadow DOM).
    const containerRef = useRef<HTMLDivElement>(null);
    // Monotonic per-item version. Pierre's controlled reconciler reuses a record
    // with the same id unless its `version` changes, so we bump the version
    // whenever an item's render signature (file content + its notes + flash)
    // changes. Keyed by item id; survives reparses.
    const itemVersions = useRef<Map<string, { sig: string; version: number }>>(new Map());

    // Parse is the expensive step — memoize on raw text only, independent of
    // note churn. Annotation attachment (cheap) re-runs when notes change.
    const committed = useMemo(() => buildSectionItems('committed', committedDiff), [committedDiff]);
    const uncommitted = useMemo(() => buildSectionItems('uncommitted', uncommittedDiff), [uncommittedDiff]);

    const activeFindHeaderMatch = activeFindMatch?.kind === 'diff-file-header'
      ? activeFindMatch
      : null;
    const isActiveFindHeaderItem = (itemId: string) => activeFindHeaderMatch?.itemId === itemId;
    const activeFindHeaderKey = activeFindHeaderMatch
      ? `${activeFindHeaderMatch.itemId}:${activeFindHeaderMatch.startColumn}:${activeFindHeaderMatch.endColumn}`
      : null;

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
          const sig = `${itemRenderSignature(it.fileDiff, notes, section, highlightedNoteId)}|find:${isActiveFindHeaderItem(it.id) ? activeFindHeaderKey : ''}`;
          const prev = vmap.get(it.id);
          const version = prev && prev.sig === sig ? prev.version : (prev?.version ?? 0) + 1;
          if (!prev || prev.sig !== sig) vmap.set(it.id, { sig, version });
          return anns.length > 0 ? { ...it, annotations: anns, version } : { ...it, version };
        });
      return [...attach(committed.items), ...attach(uncommitted.items)];
    }, [committed.items, uncommitted.items, notes, highlightedNoteId, activeFindHeaderKey]);

    const findLineDecorationCss = useMemo(() => diffFindDecorationCSS(findMatches, activeFindMatch), [findMatches, activeFindMatch]);

    const annotateLine = useCallback(
      (section: DiffSection, fileDiff: FileDiffMetadata, side: AnnotationSide, lineNumber: number) => {
        // Quote the text actually under the cursor/finger (the clicked side),
        // then resolve the anchor: a context line touched on the deletions pane
        // is normalised to its new-file number so it isn't mislabeled "Removed".
        const lineContent = lineTextAt(fileDiff, side, lineNumber) ?? '';
        const { newLine, oldLine } = resolveDiffAnchorLine(fileDiff, side, lineNumber);
        onAnnotateLine({ section, filePath: fileDiff.name, newLine, oldLine, lineContent });
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
        unsafeCSS: findLineDecorationCss,
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
      }),
      [theme, diffStyle, findLineDecorationCss, annotateLine],
    );

    // Touch long-press → annotate the line under the finger. Pierre fires
    // onLineEnter only for mouse pointer-moves, so a stationary touch never
    // records a hovered line; instead we capture the press's composed path and
    // resolve the line from it (against Pierre's rendered item containers) when
    // a 500ms hold with no movement fires.
    useEffect(() => {
      const el = containerRef.current;
      if (!el) return undefined;
      const MOVE_CANCEL_PX = 10;
      const HOLD_MS = 500;
      let timer: ReturnType<typeof setTimeout> | undefined;
      let startX = 0;
      let startY = 0;
      let downPath: EventTarget[] = [];
      const clear = () => {
        if (timer) clearTimeout(timer);
        timer = undefined;
        downPath = [];
      };
      const onDown = (e: PointerEvent) => {
        if (e.pointerType !== 'touch') return;
        startX = e.clientX;
        startY = e.clientY;
        // composedPath() is only valid during dispatch — snapshot it now.
        downPath = e.composedPath();
        if (timer) clearTimeout(timer);
        timer = setTimeout(() => {
          const rendered = (codeViewRef.current?.getInstance()?.getRenderedItems() ?? []).map(
            (r) => ({ element: r.element, item: r.item as PhoenixDiffItem }),
          );
          const target = resolveTouchedLine(downPath, rendered);
          if (!target) return;
          const section = sectionFromItemId(target.item.id);
          if (!section) return;
          annotateLine(section, target.item.fileDiff, target.side, target.lineNumber);
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
          <span
            className={`phoenix-diff-section-badge phoenix-diff-section-badge--${section}${isActiveFindHeaderItem(item.id) ? ' phoenix-diff-section-badge--find-active' : ''}`}
          >
            {section}
          </span>
        );

    }, [isActiveFindHeaderItem]);

    const renderHeaderMetadata = useCallback(
      (item: CodeViewItem<Meta>) => {
        if (item.type !== 'diff') return null;
        const section = sectionFromItemId(item.id);
        if (!section) return null;
        const filePath = item.fileDiff.name;
        const fileNotes = fileNotesFor(notes, section, filePath);
        const count = fileNotes.length;
        // File-level notes have no inline annotation to flash, so a jump from
        // the panel highlights the header affordance itself.
        const flash = fileNotes.some((n) => n.id === highlightedNoteId);
        return (
          <button
            type="button"
            className={`phoenix-diff-file-note-btn${flash ? ' phoenix-diff-file-note-btn--flash' : ''}${isActiveFindHeaderItem(item.id) ? ' phoenix-diff-file-note-btn--find-active' : ''}`}
            onClick={() => onAnnotateFile(section, filePath)}
            aria-label={`Add file-level note to ${filePath}`}
            title="Add file-level note"
          >
            <MessageSquarePlus size={14} />
            {count > 0 && <span className="phoenix-diff-file-note-count">{count}</span>}
          </button>
        );
      },
      [notes, highlightedNoteId, isActiveFindHeaderItem, onAnnotateFile],
    );

    const renderGutterUtility = useCallback(
      (getHoveredLine: () => { lineNumber: number; side: AnnotationSide } | undefined, item: CodeViewItem<Meta>) => {
        if (item.type !== 'diff') return null;
        const fileDiff = item.fileDiff;
        const id = item.id;
        return (
          <button
            type="button"
            data-utility-button=""
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
          if (!target) return;
          scrollCodeViewToTarget(codeViewRef.current, target);
        },
        scrollToFindTarget: (target: DiffSearchMatchTarget) => {
          if (target.kind === 'diff-line' && target.lineNumber && target.side) {
            scrollCodeViewToTarget(codeViewRef.current, {
              id: target.itemId,
              line: { lineNumber: target.lineNumber, side: target.side },
            });
            return;
          }
          scrollCodeViewToTarget(codeViewRef.current, { id: target.itemId });
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

function scrollCodeViewToTarget(
  codeView: CodeViewHandle<Meta> | null,
  target: { id: string; line?: { lineNumber: number; side: AnnotationSide } },
) {
  if (!codeView) return;
  if (target.line) {
    codeView.scrollTo({
      type: 'line',
      id: target.id,
      lineNumber: target.line.lineNumber,
      side: target.line.side,
      align: 'center',
      behavior: 'smooth',
    });
    return;
  }
  codeView.scrollTo({ type: 'item', id: target.id, align: 'start', behavior: 'smooth' });
}

/**
 * Pierre exposes diff-line annotations plus whole-row CSS injection (`unsafeCSS`),
 * but no typed substring-decoration API on rendered diff rows. We therefore use
 * the strongest supported primitive here: decorate every matched rendered line,
 * and intensify the active line. File-header matches are surfaced via Phoenix's
 * typed header slots instead of DOM scraping.
 */
function diffFindDecorationCSS(
  matches: readonly DiffSearchMatchTarget[],
  activeMatch: DiffSearchMatchTarget | null,
): string {
  const grouped = new Map<string, Set<number>>();
  for (const match of matches) {
    if (match.kind !== 'diff-line' || match.lineNumber === undefined) continue;
    const key = `${match.itemId}:${match.side ?? 'additions'}`;
    const lines = grouped.get(key) ?? new Set<number>();
    lines.add(match.lineNumber);
    grouped.set(key, lines);
  }
  const rules: string[] = [];
  for (const [key, lines] of grouped) {
    const divider = key.lastIndexOf(':');
    const itemId = key.slice(0, divider);
    const side = key.slice(divider + 1);
    const lineSelector = [...lines].sort((a, b) => a - b).map((line) => lineSelectorFor(itemId, side, line)).join(',');
    if (lineSelector) rules.push(`${lineSelector}{background:var(--viewer-modified-line-bg);}`);
  }
  if (activeMatch?.kind === 'diff-line' && activeMatch.lineNumber !== undefined) {
    rules.push(`${lineSelectorFor(activeMatch.itemId, activeMatch.side ?? 'additions', activeMatch.lineNumber)}{background:var(--viewer-highlight-line-bg);}`);
  }
  return rules.join('\n');
}

function lineSelectorFor(itemId: string, side: string, line: number): string {
  return `[data-item-id="${cssEscape(itemId)}"] [data-${side}=""] [data-line="${line}"]`;
}

function cssEscape(value: string): string {
  return value.replaceAll('\\', '\\\\').replaceAll('"', '\\"');
}
