/**
 * MetaViewer — the central viewer router.
 *
 * Takes a *resolved* `MetaViewerPayload` (the loader has already fetched and
 * classified the content) and composes the shared `ViewerShell` chrome with the
 * matching body renderer. It owns the cross-cutting concerns that are the same
 * regardless of render kind — scroll restoration, copy-newline normalization,
 * select-all, jump-to-line, and the HTML source/preview toggle — and delegates
 * pure rendering to the body components.
 *
 * Review-note lifecycle for text-like payloads lives in `useFileReviewNotes`.
 * Image payloads carry no notes.
 */
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { useRegisterFocusScope } from '../../hooks/useFocusScope';
import { ViewerShell } from './ViewerShell';
import { NotesPanel } from './NotesPanel';
import { AnnotationDialog } from './AnnotationDialog';
import { CopyButton } from '../CopyButton';
import { useFileReviewNotes } from './useFileReviewNotes';
import { isTextLikePayload } from './metaViewerTypes';
import type { MetaViewerPayload, PatchContext } from './metaViewerTypes';
import type { ReviewNote } from '../../contexts/ReviewNotesContext';
import { MarkdownViewerBody } from './MarkdownViewerBody';
import { CodeViewerBody } from './CodeViewerBody';
import { TextViewerBody } from './TextViewerBody';
import { HtmlViewerBody } from './HtmlViewerBody';
import type { HtmlViewMode } from './HtmlViewerBody';
import { ImageViewerBody } from './ImageViewerBody';
import type { ViewerBodyProps } from './AnnotatableBlock';

export function MetaViewer({ payload }: { payload: MetaViewerPayload }) {
  useRegisterFocusScope('prose-reader');

  const { absolutePath, title, onClose, onSendNotes, inline } = payload;
  const textLike = isTextLikePayload(payload);
  const content = textLike ? payload.content : '';
  const patchContext: PatchContext | undefined = textLike ? payload.patchContext : undefined;

  const notes = useFileReviewNotes(absolutePath, onSendNotes, patchContext);

  const [htmlViewMode, setHtmlViewMode] = useState<HtmlViewMode>('source');
  const lineRefs = useRef<Map<number, HTMLElement>>(new Map());
  const contentRef = useRef<HTMLDivElement>(null);
  const scrollRestoredRef = useRef(false);
  const lastScrollTopRef = useRef(0);

  const scrollKey = useMemo(() => `phoenix:prose-scroll:${absolutePath}`, [absolutePath]);

  const registerLineRef = useCallback((lineNumber: number, el: HTMLElement | null) => {
    if (el) lineRefs.current.set(lineNumber, el);
    else lineRefs.current.delete(lineNumber);
  }, []);

  // New file → new restoration target.
  useEffect(() => {
    scrollRestoredRef.current = false;
    lastScrollTopRef.current = 0;
  }, [scrollKey]);

  // Restore saved scroll position before paint. Always runs (regardless of
  // patchContext) so the patchContext auto-scroll below has a fallback when it
  // can't locate firstModifiedLine. Don't mark scrollRestoredRef here — leave
  // it false so the patchContext effect can still win.
  useLayoutEffect(() => {
    if (!content) return;
    if (scrollRestoredRef.current) return;
    const saved = (() => {
      try { return localStorage.getItem(scrollKey); } catch { return null; }
    })();
    if (saved !== null) {
      const pos = parseInt(saved, 10);
      if (!Number.isNaN(pos)) {
        const el = contentRef.current;
        if (el) {
          el.scrollTop = pos;
          lastScrollTopRef.current = pos;
        }
      }
    }
  }, [content, scrollKey]);

  // Auto-scroll to first modified line. Wins over saved scroll when a
  // patchContext is provided and the line element exists.
  useEffect(() => {
    if (!content || !patchContext?.firstModifiedLine) return undefined;
    const timer = setTimeout(() => {
      const lineEl = lineRefs.current.get(patchContext.firstModifiedLine!);
      if (lineEl) {
        lineEl.scrollIntoView({ behavior: 'smooth', block: 'center' });
        scrollRestoredRef.current = true;
      }
    }, 100);
    return () => clearTimeout(timer);
  }, [content, patchContext?.firstModifiedLine]);

  // Track scrollTop so visibility-change / unmount saves see the latest value.
  useEffect(() => {
    const el = contentRef.current;
    if (!el) return;
    const onScroll = () => { lastScrollTopRef.current = el.scrollTop; };
    el.addEventListener('scroll', onScroll, { passive: true });
    return () => el.removeEventListener('scroll', onScroll);
  }, [content]);

  // Persist scroll on backgrounding and unmount.
  useEffect(() => {
    const save = () => {
      try { localStorage.setItem(scrollKey, String(lastScrollTopRef.current)); } catch { /* storage full */ }
    };
    const onVis = () => { if (document.visibilityState === 'hidden') save(); };
    document.addEventListener('visibilitychange', onVis);
    return () => {
      document.removeEventListener('visibilitychange', onVis);
      save();
    };
  }, [scrollKey]);

  // Cmd/Ctrl+A selects the viewer body, unless an editable element is focused.
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (!((e.ctrlKey || e.metaKey) && e.key === 'a')) return;
      const target = e.target as HTMLElement | null;
      if (target) {
        const tag = target.tagName;
        if (tag === 'INPUT' || tag === 'TEXTAREA' || target.isContentEditable) return;
      }
      const container = contentRef.current;
      if (container) {
        e.preventDefault();
        const range = document.createRange();
        range.selectNodeContents(container);
        const sel = window.getSelection();
        sel?.removeAllRanges();
        sel?.addRange(range);
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, []);

  // Copy: collapse double newlines introduced by block-element boundaries.
  useEffect(() => {
    const container = contentRef.current;
    if (!container) return undefined;
    const handleCopy = (e: ClipboardEvent) => {
      const sel = window.getSelection();
      if (!sel || sel.rangeCount === 0) return;
      const range = sel.getRangeAt(0);
      if (!container.contains(range.startContainer)) return;
      const cleaned = sel.toString().replace(/\n\n/g, '\n');
      e.preventDefault();
      e.clipboardData?.setData('text/plain', cleaned);
    };
    container.addEventListener('copy', handleCopy);
    return () => container.removeEventListener('copy', handleCopy);
  }, []);

  // Jump-to-line lives here, not in the notes hook, because it needs the DOM
  // refs the rendered body registers into `lineRefs`.
  const { highlight, closePanel } = notes;
  const handleJumpTo = useCallback(
    (note: ReviewNote) => {
      if (note.anchor.kind !== 'file' || note.anchor.filePath !== absolutePath) return;
      const el = lineRefs.current.get(note.anchor.lineNumber);
      if (el) {
        el.scrollIntoView({ behavior: 'smooth', block: 'center' });
        highlight(note.anchor.lineNumber);
      }
      closePanel();
    },
    [absolutePath, highlight, closePanel],
  );

  const modifiedLines = patchContext?.modifiedLines ?? EMPTY_SET;
  const bodyProps: ViewerBodyProps = {
    content,
    modifiedLines,
    highlightedLine: notes.highlightedLine,
    onAnnotate: notes.startAnnotate,
    registerLineRef,
  };

  const body = renderBody(payload, bodyProps, htmlViewMode);

  const headerExtras: ReactNode = textLike ? (
    <>
      <CopyButton text={content} className="viewer-shell-copy-btn" title="Copy file contents" />
      {payload.kind === 'html' && (
        <>
          <button
            type="button"
            className={`viewer-shell-toggle ${htmlViewMode === 'preview' ? 'active' : ''}`}
            onClick={() => setHtmlViewMode(htmlViewMode === 'preview' ? 'source' : 'preview')}
            title={htmlViewMode === 'preview' ? 'Show source' : 'Show sandboxed preview (no scripts)'}
          >
            {htmlViewMode === 'preview' ? '</>' : 'Preview'}
          </button>
          <a
            className="viewer-shell-toggle"
            href={payload.previewUrl}
            target="_blank"
            rel="noopener noreferrer"
            title="Open in new tab (full render with scripts)"
          >
            Open in browser
          </a>
        </>
      )}
    </>
  ) : null;

  const banner: ReactNode =
    textLike && patchContext && patchContext.modifiedLines.size > 0 ? (
      <span>
        Viewing {title}: {patchContext.modifiedLines.size} change
        {patchContext.modifiedLines.size !== 1 ? 's' : ''} from patch
      </span>
    ) : null;

  return (
    <ViewerShell
      mode={inline ? 'inline' : 'overlay'}
      ariaLabel={`File viewer: ${title}`}
      title={title}
      titleTooltip={absolutePath}
      headerExtras={headerExtras}
      noteCount={notes.fileNotes.length}
      onToggleNotes={notes.togglePanel}
      onSend={notes.send}
      banner={banner}
      onClose={onClose}
      panel={
        notes.showPanel ? (
          <NotesPanel
            notes={notes.fileNotes}
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
            anchorLabel={`Line ${notes.annotating.lineNumber}`}
            lineContent={notes.annotating.lineContent}
            onSubmit={notes.submitNote}
            onCancel={notes.cancelAnnotate}
          />
        ) : null
      }
    >
      <div className="prose-reader-content" ref={contentRef}>
        {body}
      </div>
    </ViewerShell>
  );
}

const EMPTY_SET: Set<number> = new Set();

function renderBody(
  payload: MetaViewerPayload,
  bodyProps: ViewerBodyProps,
  htmlViewMode: HtmlViewMode,
): ReactNode {
  switch (payload.kind) {
    case 'markdown':
      return <MarkdownViewerBody {...bodyProps} />;
    case 'code':
      return <CodeViewerBody {...bodyProps} language={payload.language} />;
    case 'html':
      return (
        <HtmlViewerBody
          {...bodyProps}
          mode={htmlViewMode}
          language={payload.language}
          previewUrl={payload.previewUrl}
        />
      );
    case 'text':
      return <TextViewerBody {...bodyProps} />;
    case 'image':
      return <ImageViewerBody fileName={payload.fileName} url={payload.url} />;
  }
}
