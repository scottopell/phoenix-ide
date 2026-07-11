/**
 * PhoenixFileCodeView — the single boundary between Phoenix and `@pierre/diffs`
 * for the *file* (non-diff) viewer. The diff counterpart is
 * `PhoenixDiffCodeView`; this renders one syntax-highlighted, virtualized file
 * through Pierre's `CodeView` file item, maps Phoenix `kind: 'file'` notes
 * to/from Pierre line annotations, shades patch-modified lines via `unsafeCSS`,
 * and exposes a typed `scrollToLine` for jump-to-line. Pierre types stay behind
 * this wrapper; no annotation, indicator, line identity, or scroll path scrapes
 * Pierre's rendered DOM (the sole exception being the touch long-press line
 * resolver in `pierreFileMapping`, which has no typed Pierre callback).
 *
 * Note affordances mirror the diff viewer: gutter `+` on the hovered line
 * (pointer), click-to-annotate (mouse/pen), and a 500ms long-press (touch).
 * Modified-line shading and jump-target flashing are CSS decorations keyed to
 * Pierre's `data-line` attribute, not note annotations.
 */

import { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useRef } from 'react';
import { MessageSquarePlus } from 'lucide-react';
import { CodeView } from '@pierre/diffs/react';
import type { CodeViewHandle, CodeViewItem } from '@pierre/diffs/react';
import type { CodeViewOptions } from '@pierre/diffs';
import type { ReviewNote } from '../../contexts/ReviewNotesContext';
import { useTheme } from '../../hooks/useTheme';
import {
  annotationsForFile,
  buildFileItem,
  contentFingerprint,
  fileItemId,
  fileItemRenderSignature,
  lineDecorationCSS,
  lineTextAt,
  resolveTouchedLineNumber,
  type PhoenixFileAnnotationMeta,
  type PhoenixFileItem,
} from './pierreFileMapping';
import type { FileSearchMatchTarget } from '../viewer-find';

export interface PhoenixFileCodeViewProps {
  filePath: string;
  content: string;
  /** File-scoped review notes for this path (already filtered by the caller). */
  notes: readonly ReviewNote[];
  /** Lines to shade as patch-modified, or empty. */
  modifiedLines: ReadonlySet<number>;
  /** Line to flash after a jump, or null. */
  highlightedLine: number | null;
  /** Line to auto-scroll to on first open (patch provenance), or undefined. */
  firstModifiedLine?: number | undefined;
  /** localStorage key for per-file scroll restoration. */
  scrollKey: string;
  /** Open the annotation dialog for a line, quoting its source text. */
  onAnnotateLine: (lineNumber: number, lineContent: string) => void;
  findMatches?: readonly FileSearchMatchTarget[] | undefined;
  activeFindMatch?: FileSearchMatchTarget | null | undefined;
}

export interface PhoenixFileCodeViewHandle {
  scrollToLine: (lineNumber: number) => void;
  scrollToFindTarget: (target: FileSearchMatchTarget) => void;
}

type Meta = PhoenixFileAnnotationMeta;

export const PhoenixFileCodeView = forwardRef<PhoenixFileCodeViewHandle, PhoenixFileCodeViewProps>(
  function PhoenixFileCodeView(
    { filePath, content, notes, modifiedLines, highlightedLine, firstModifiedLine, scrollKey, onAnnotateLine, findMatches = [], activeFindMatch = null },
    ref,
  ) {
    const { theme } = useTheme();
    const codeViewRef = useRef<CodeViewHandle<Meta>>(null);
    const containerRef = useRef<HTMLDivElement>(null);
    // Monotonic per-item version; bumped whenever the item's render signature
    // (content + notes + modified lines + flash) changes so Pierre's controlled
    // reconciler re-renders instead of keeping a stale annotation/decoration.
    const itemVersion = useRef<{ sig: string; version: number }>({ sig: '', version: 0 });
    const lastScrollTop = useRef(0);

    // Parse/build is keyed on text only, independent of note churn.
    const baseItem = useMemo(() => buildFileItem(filePath, content), [filePath, content]);
    // Hashing scans the whole file — memoize on content so note-only updates
    // (which recompute the signature) don't re-hash a huge file every edit.
    const contentFp = useMemo(() => contentFingerprint(content), [content]);

    const item = useMemo<PhoenixFileItem>(() => {
      const anns = annotationsForFile(notes, filePath);
      // File-viewer flash is line-driven (the jumped-to line), not a note id, so
      // the note-flash channel is unused; highlightedLine carries the flash.
      const sig = fileItemRenderSignature(filePath, contentFp, notes, modifiedLines, null, highlightedLine);
      const prev = itemVersion.current;
      const version = prev.sig === sig ? prev.version : prev.version + 1;
      if (prev.sig !== sig) itemVersion.current = { sig, version };
      return anns.length > 0 ? { ...baseItem, annotations: anns, version } : { ...baseItem, version };
    }, [baseItem, filePath, contentFp, notes, modifiedLines, highlightedLine]);

    const items = useMemo(() => [item], [item]);

    const annotate = useCallback(
      (lineNumber: number) => onAnnotateLine(lineNumber, lineTextAt(content, lineNumber)),
      [onAnnotateLine, content],
    );

    const options = useMemo<CodeViewOptions<Meta>>(
      () => ({
        theme: { dark: 'pierre-dark', light: 'pierre-light' },
        themeType: theme,
        stickyHeaders: true,
        enableGutterUtility: true,
        unsafeCSS: fileFindDecorationCSS(modifiedLines, highlightedLine, findMatches, activeFindMatch),
        // Items are always file items here; narrow on context.type and cast
        // through `unknown` to satisfy the file+diff overload intersection
        // without re-stating both shapes (see PhoenixDiffCodeView).
        onLineClick: ((
          props: { lineNumber: number; event?: { pointerType?: string } },
          context: { type: 'diff' | 'file' },
        ) => {
          if (context.type !== 'file') return;
          // Touch annotates via long-press, not tap.
          if (props.event?.pointerType === 'touch') return;
          annotate(props.lineNumber);
        }) as unknown as NonNullable<CodeViewOptions<Meta>['onLineClick']>,
      }),
      [theme, modifiedLines, highlightedLine, annotate, findMatches, activeFindMatch],
    );

    // Touch long-press → annotate the line under the finger (Pierre fires
    // onLineEnter for mouse moves only, so a stationary touch records no hover).
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
        downPath = e.composedPath();
        if (timer) clearTimeout(timer);
        timer = setTimeout(() => {
          const lineNumber = resolveTouchedLineNumber(downPath);
          if (lineNumber !== undefined) annotate(lineNumber);
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
    }, [annotate]);

    const renderAnnotation = useCallback(
      (annotation: { metadata?: Meta }) => {
        const meta = annotation.metadata;
        if (!meta) return null;
        const note = notes.find((n) => n.id === meta.noteId);
        if (!note) return null;
        const flash = note.anchor.kind === 'file' && note.anchor.lineNumber === highlightedLine;
        return (
          <div className={`phoenix-file-note${flash ? ' phoenix-file-note--flash' : ''}`} role="note">
            <span className="phoenix-file-note-body">{note.body}</span>
          </div>
        );
      },
      [notes, highlightedLine],
    );

    const renderGutterUtility = useCallback(
      (getHoveredLine: () => { lineNumber: number } | undefined, codeItem: CodeViewItem<Meta>) => {
        if (codeItem.type !== 'file') return null;
        return (
          <button
            type="button"
            data-utility-button=""
            className="phoenix-file-add-note"
            aria-label="Add note to line"
            title="Add note"
            onClick={(e) => {
              e.stopPropagation();
              const hovered = getHoveredLine();
              if (hovered) annotate(hovered.lineNumber);
            }}
          >
            <MessageSquarePlus size={12} />
          </button>
        );
      },
      [annotate],
    );

    useImperativeHandle(
      ref,
      () => ({
        scrollToLine: (lineNumber: number) => {
          codeViewRef.current?.scrollTo({
            type: 'line',
            id: fileItemId(filePath),
            lineNumber,
            align: 'center',
            behavior: 'smooth',
          });
        },
        scrollToFindTarget: (target: FileSearchMatchTarget) => {
          codeViewRef.current?.scrollTo({
            type: 'line',
            id: fileItemId(filePath),
            lineNumber: target.lineNumber,
            align: 'center',
            behavior: 'smooth',
          });
        },
      }),
      [filePath],
    );

    // Scroll restoration / patch auto-scroll. A patch's first modified line wins
    // over saved position; otherwise restore the saved scrollTop. Best-effort:
    // runs once after layout settles, keyed to the file.
    useEffect(() => {
      lastScrollTop.current = 0;
      const cv = codeViewRef.current;
      if (!cv) return undefined;
      const timer = setTimeout(() => {
        if (firstModifiedLine) {
          cv.scrollTo({ type: 'line', id: fileItemId(filePath), lineNumber: firstModifiedLine, align: 'center' });
          return;
        }
        const saved = (() => {
          try { return localStorage.getItem(scrollKey); } catch { return null; }
        })();
        if (saved !== null) {
          const pos = Number.parseInt(saved, 10);
          if (!Number.isNaN(pos)) {
            cv.scrollTo({ type: 'position', position: pos });
            // Seed lastScrollTop with the restored value so a close/background
            // before Pierre's first onScroll doesn't save 0 over the position.
            lastScrollTop.current = pos;
          }
        }
      }, 50);
      return () => clearTimeout(timer);
    }, [scrollKey, filePath, firstModifiedLine]);

    // Persist scroll position on backgrounding and unmount.
    useEffect(() => {
      const save = () => {
        try { localStorage.setItem(scrollKey, String(lastScrollTop.current)); } catch { /* storage full */ }
      };
      const onVis = () => { if (document.visibilityState === 'hidden') save(); };
      document.addEventListener('visibilitychange', onVis);
      return () => {
        document.removeEventListener('visibilitychange', onVis);
        save();
      };
    }, [scrollKey]);

    const onScroll = useCallback((scrollTop: number) => { lastScrollTop.current = scrollTop; }, []);

    return (
      <div className="phoenix-file-codeview-wrap">
        <CodeView<Meta>
          ref={codeViewRef}
          containerRef={containerRef}
          items={items}
          options={options}
          className="phoenix-file-codeview"
          renderAnnotation={renderAnnotation}
          renderGutterUtility={renderGutterUtility}
          onScroll={onScroll}
        />
      </div>
    );
  },
);

function fileFindDecorationCSS(
  modifiedLines: ReadonlySet<number>,
  highlightedLine: number | null,
  findMatches: readonly FileSearchMatchTarget[],
  activeFindMatch: FileSearchMatchTarget | null,
): string {
  const base = lineDecorationCSS(modifiedLines, highlightedLine);
  const matchedLines = [...new Set(findMatches.map((match) => match.lineNumber))]
    .filter((lineNumber) => lineNumber !== highlightedLine)
    .sort((a, b) => a - b);
  const rules = [base].filter((rule) => rule.length > 0);
  if (matchedLines.length > 0) {
    rules.push(`${matchedLines.map((lineNumber) => `[data-line="${lineNumber}"]`).join(',')}{outline:1px solid var(--viewer-find-match-outline, rgba(240, 180, 41, 0.8));outline-offset:-1px;}`);
  }
  if (activeFindMatch) {
    rules.push(`[data-line="${activeFindMatch.lineNumber}"]{background:var(--viewer-highlight-line-bg);outline:2px solid var(--viewer-find-active-outline, rgba(255, 215, 64, 0.95));outline-offset:-2px;}`);
  }
  return rules.join('\n');
}
