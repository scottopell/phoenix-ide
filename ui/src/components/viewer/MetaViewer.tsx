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
import { createPortal } from 'react-dom';
import { Maximize2, Minimize2 } from 'lucide-react';
import { useRegisterFocusScope } from '../../hooks/useFocusScope';
import { ViewerShell } from './ViewerShell';
import { FindBar, buildFileSearchProjection, useViewerFind, useViewerFindKeyboardShortcut } from '../viewer-find';
import type { FileSearchProjection } from '../viewer-find';
import { NotesPanel } from './NotesPanel';
import { AnnotationDialog } from './AnnotationDialog';
import { CopyButton } from '../CopyButton';
import { useFileReviewNotes } from './useFileReviewNotes';
import { isTextLikePayload } from './metaViewerTypes';
import type { MetaViewerPayload, PatchContext } from './metaViewerTypes';
import type { ReviewNote } from '../../contexts/ReviewNotesContext';
import { MarkdownViewerBody } from './MarkdownViewerBody';
import { PhoenixFileCodeView } from './PhoenixFileCodeView';
import type { PhoenixFileCodeViewHandle } from './PhoenixFileCodeView';
import { TextViewerBody } from './TextViewerBody';
import { HtmlViewerBody } from './HtmlViewerBody';
import type { HtmlViewMode } from './HtmlViewerBody';
import { ImageViewerBody } from './ImageViewerBody';
import type { ViewerBodyProps } from './AnnotatableBlock';

export function MetaViewer({ payload }: { payload: MetaViewerPayload }) {
  useRegisterFocusScope('file-viewer');

  const { absolutePath, title, onClose, onSendNotes, inline, focusLine } = payload;
  const textLike = isTextLikePayload(payload);
  const content = textLike ? payload.content : '';
  const patchContext: PatchContext | undefined = textLike ? payload.patchContext : undefined;

  const notes = useFileReviewNotes(absolutePath, onSendNotes, patchContext);

  // Pierre-backed payloads own their virtualized scroller and line identity — the
  // lineRef/contentRef machinery below (scroll restore, jump-to-line, select-all,
  // copy) is bypassed for them and handled by PhoenixFileCodeView via its typed
  // handle instead.
  const usePierreCode = payload.kind === 'code' || payload.kind === 'text';
  const fileCodeRef = useRef<PhoenixFileCodeViewHandle>(null);

  const [htmlViewMode, setHtmlViewMode] = useState<HtmlViewMode>('source');
  const [imageTakeover, setImageTakeover] = useState(false);
  const findButtonRef = useRef<HTMLButtonElement>(null);
  const lineRefs = useRef<Map<number, HTMLElement>>(new Map());
  const contentRef = useRef<HTMLDivElement>(null);
  const scrollRestoredRef = useRef(false);
  const lastScrollTopRef = useRef(0);
  const findPreviousFocusRef = useRef<HTMLElement | null>(null);

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
    // PhoenixFileCodeView owns scroll for Pierre-backed payloads under the same
    // scrollKey — let it handle restore so the two don't fight.
    if (usePierreCode) return;
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
  }, [content, scrollKey, usePierreCode]);

  // Auto-scroll to first modified line. Wins over saved scroll when a
  // patchContext is provided and the line element exists.
  useEffect(() => {
    // Pierre-backed patch auto-scroll is handled inside PhoenixFileCodeView.
    if (usePierreCode) return undefined;
    if (!content || !patchContext?.firstModifiedLine) return undefined;
    const timer = setTimeout(() => {
      const lineEl = lineRefs.current.get(patchContext.firstModifiedLine!);
      if (lineEl) {
        lineEl.scrollIntoView({ behavior: 'smooth', block: 'center' });
        scrollRestoredRef.current = true;
      }
    }, 100);
    return () => clearTimeout(timer);
  }, [content, patchContext?.firstModifiedLine, usePierreCode]);

  // Track scrollTop so visibility-change / unmount saves see the latest value.
  useEffect(() => {
    if (usePierreCode) return;
    const el = contentRef.current;
    if (!el) return;
    const onScroll = () => { lastScrollTopRef.current = el.scrollTop; };
    el.addEventListener('scroll', onScroll, { passive: true });
    return () => el.removeEventListener('scroll', onScroll);
  }, [content, usePierreCode]);

  // Persist scroll on backgrounding and unmount. Skipped for Pierre-backed
  // payloads: the wrapper persists under the same scrollKey, and the parent's
  // contentRef never scrolls, so saving here would clobber the real position with 0.
  useEffect(() => {
    if (usePierreCode) return;
    const save = () => {
      try { localStorage.setItem(scrollKey, String(lastScrollTopRef.current)); } catch { /* storage full */ }
    };
    const onVis = () => { if (document.visibilityState === 'hidden') save(); };
    document.addEventListener('visibilitychange', onVis);
    return () => {
      document.removeEventListener('visibilitychange', onVis);
      save();
    };
  }, [scrollKey, usePierreCode]);

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

  const { highlight, closePanel } = notes;

  // Auto-scroll to a search/jump target line. Runs after file content is loaded
  // and flashes the line without creating a review note.
  useEffect(() => {
    if (!content || !focusLine) return undefined;
    const timer = setTimeout(() => {
      if (usePierreCode) {
        fileCodeRef.current?.scrollToLine(focusLine);
        highlight(focusLine);
      } else {
        const lineEl = lineRefs.current.get(focusLine);
        if (lineEl) {
          lineEl.scrollIntoView({ behavior: 'smooth', block: 'center' });
          highlight(focusLine);
        }
      }
    }, 100);
    return () => clearTimeout(timer);
  }, [content, focusLine, highlight, usePierreCode]);

  // Jump-to-line lives here, not in the notes hook, because it needs the DOM
  // refs the rendered body registers into `lineRefs`.
  const handleJumpTo = useCallback(
    (note: ReviewNote) => {
      if (note.anchor.kind !== 'file' || note.anchor.filePath !== absolutePath) return;
      // Pierre-backed payloads render in their own scroller; jump via the typed
      // handle. Other bodies expose DOM line refs the viewer scrolls directly.
      if (usePierreCode) {
        fileCodeRef.current?.scrollToLine(note.anchor.lineNumber);
        highlight(note.anchor.lineNumber);
      } else {
        const el = lineRefs.current.get(note.anchor.lineNumber);
        if (el) {
          el.scrollIntoView({ behavior: 'smooth', block: 'center' });
          highlight(note.anchor.lineNumber);
        }
      }
      closePanel();
    },
    [absolutePath, highlight, closePanel, usePierreCode],
  );

  // The plain-text fallback never applies to HTML preview: preview renders an
  // iframe (no per-line DOM cost), so a large HTML file must still reach it
  // rather than being stranded on the raw <pre>.
  const htmlPreview = payload.kind === 'html' && htmlViewMode === 'preview';
  const largeFallback = textLike && !usePierreCode && payload.renderMode === 'plainLargeText' && !htmlPreview;

  const findEligible = (textLike && !htmlPreview) || largeFallback;
  const findSourceText = findEligible ? content : '';
  const find = useViewerFind({ text: findSourceText });
  const shouldProjectFind = findEligible && find.isOpen && find.query.length > 0;
  const findProjection = useMemo<FileSearchProjection>(
    () => (shouldProjectFind ? buildFileSearchProjection(findSourceText, find.query) : { sources: [], matches: [] }),
    [find.query, findSourceText, shouldProjectFind],
  );
  const activeFindMatch = find.isOpen && find.activeIndex >= 0 ? findProjection.matches[find.activeIndex]?.target ?? null : null;
  const findMatchTargets = useMemo(
    () => (find.isOpen ? findProjection.matches.map((match) => match.target) : []),
    [find.isOpen, findProjection.matches],
  );

  const modifiedLines = patchContext?.modifiedLines ?? EMPTY_SET;
  const bodyProps: ViewerBodyProps = {
    content,
    modifiedLines,
    highlightedLine: notes.highlightedLine,
    onAnnotate: notes.startAnnotate,
    registerLineRef,
    findQuery: find.isOpen ? find.query : '',
    activeFindOccurrence: find.isOpen ? find.activeIndex : null,
  };
  const body = usePierreCode ? null : renderBody(payload, bodyProps, htmlViewMode, imageTakeover ? 'takeover' : 'pane');

  useEffect(() => {
    if (!find.isOpen || !activeFindMatch) return;
    if (usePierreCode) {
      fileCodeRef.current?.scrollToFindTarget(activeFindMatch);
      return;
    }
    const selector = `[data-find-occurrence="${find.activeIndex}"]`;
    const matchEl = contentRef.current?.querySelector<HTMLElement>(selector);
    if (matchEl) {
      matchEl.scrollIntoView({ behavior: 'smooth', block: 'center' });
      return;
    }
    const lineEl = lineRefs.current.get(activeFindMatch.lineNumber);
    lineEl?.scrollIntoView({ behavior: 'smooth', block: 'center' });
  }, [activeFindMatch, find.activeIndex, find.isOpen, usePierreCode]);

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
    scopeId: 'file-viewer',
    onOpen: openFind,
    enabled: findEligible,
    dialogOpen: notes.annotating !== null,
  });

  useEffect(() => {
    if (findEligible || !find.isOpen) return;
    find.close();
  }, [find, find.isOpen, findEligible]);

  const handleFindQueryChange = useCallback((query: string) => {
    find.setQuery(query);
    const nextProjection = findEligible ? buildFileSearchProjection(findSourceText, query) : { matches: [] };
    const target = nextProjection.matches[0]?.target;
    if (!target) return;
    if (usePierreCode) {
      fileCodeRef.current?.scrollToFindTarget(target);
      return;
    }
    queueMicrotask(() => {
      contentRef.current?.querySelector<HTMLElement>('[data-find-occurrence="0"]')?.scrollIntoView({ behavior: 'smooth', block: 'center' });
    });
  }, [find, findEligible, findSourceText, usePierreCode]);

  const handleFindNext = useCallback(() => {
    find.nextMatch();
  }, [find]);

  const handleFindPrevious = useCallback(() => {
    find.previousMatch();
  }, [find]);

  const headerExtras: ReactNode = textLike ? (
    <>
      {findEligible && (
        <button
          ref={findButtonRef}
          type="button"
          className="viewer-shell-btn"
          onClick={openFind}
          aria-label="Find in file"
          title="Find in file"
        >
          Find
        </button>
      )}
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
  ) : payload.kind === 'image' ? (
    <>
      <a
        className="viewer-shell-toggle"
        href={payload.url}
        target="_blank"
        rel="noopener noreferrer"
        title="Open image in a browser tab"
      >
        Open in new tab
      </a>
      <button
        type="button"
        className="viewer-shell-toggle viewer-shell-icon-toggle"
        onClick={() => setImageTakeover((value) => !value)}
        aria-label={imageTakeover ? 'Exit fullscreen image viewer' : 'Open fullscreen image viewer'}
        title={imageTakeover ? 'Exit fullscreen' : 'Open fullscreen'}
      >
        {imageTakeover ? <Minimize2 size={16} /> : <Maximize2 size={16} />}
        <span>{imageTakeover ? 'Exit fullscreen' : 'Fullscreen'}</span>
      </button>
    </>
  ) : null;

  const patchChangeCount = patchContext?.modifiedLines.size ?? 0;
  const viewerBanner: ReactNode = largeFallback ? (
    <span>
      Large file shown as plain text for responsiveness. Rich highlighting and line notes are disabled.
      {patchChangeCount > 0
        ? ` Opened from a patch with ${patchChangeCount} change${patchChangeCount !== 1 ? 's' : ''} (not highlighted in this view).`
        : ''}
    </span>
  ) : textLike && patchChangeCount > 0 ? (
    <span>
      Viewing {title}: {patchChangeCount} change
      {patchChangeCount !== 1 ? 's' : ''} from patch
    </span>
  ) : null;
  const findIneligibleReason = payload.kind === 'image'
    ? 'Find unavailable for images.'
    : payload.kind === 'html' && htmlViewMode === 'preview'
      ? 'Find unavailable in HTML preview; switch to source to search file contents.'
      : null;
  const banner: ReactNode = find.isOpen ? (
    <FindBar
      query={find.query}
      activeIndex={find.activeIndex}
      matchCount={findEligible ? find.matchCount : 0}
      focusVersion={find.focusVersion}
      onQueryChange={handleFindQueryChange}
      onNext={handleFindNext}
      onPrevious={handleFindPrevious}
      onClose={closeFind}
      autoFocus
    />
  ) : viewerBanner ?? (findIneligibleReason ? <span>{findIneligibleReason}</span> : null);

  const viewerMode = payload.kind === 'image' && imageTakeover ? 'takeover' : inline ? 'inline' : 'overlay';
  const shell = (
    <ViewerShell
      closeOnEscape={!find.isOpen}
      onInnerEscape={closeFind}
      mode={viewerMode}
      ariaLabel={`File viewer: ${title}`}
      title={title}
      titleTooltip={absolutePath}
      headerExtras={headerExtras}
      noteCount={notes.fileNotes.length}
      onToggleNotes={notes.togglePanel}
      onSend={notes.send}
      banner={banner}
      onClose={onClose}
      suppressCloseButtonFocus={find.isOpen}
      bodyScroll={usePierreCode ? 'children' : 'shell'}
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
      {usePierreCode ? (
        <PhoenixFileCodeView
          ref={fileCodeRef}
          filePath={absolutePath}
          content={content}
          notes={notes.fileNotes}
          modifiedLines={modifiedLines}
          highlightedLine={notes.highlightedLine}
          firstModifiedLine={patchContext?.firstModifiedLine ?? focusLine}
          scrollKey={scrollKey}
          onAnnotateLine={notes.startAnnotate}
          findMatches={findMatchTargets}
          activeFindMatch={activeFindMatch}
        />
      ) : (
        <div className={`viewer-content ${payload.kind === 'image' ? 'viewer-content--image' : ''}`} ref={contentRef}>
          {body}
        </div>
      )}
    </ViewerShell>
  );

  return payload.kind === 'image' && imageTakeover ? createPortal(shell, document.body) : shell;
}

const EMPTY_SET: Set<number> = new Set();

function renderBody(
  payload: MetaViewerPayload,
  bodyProps: ViewerBodyProps,
  htmlViewMode: HtmlViewMode,
  imageViewKey: string,
): ReactNode {
  // HTML preview renders an iframe with no per-line cost, so the large-file
  // fallback must not pre-empt it; only source-like rendering falls back.
  const htmlPreview = payload.kind === 'html' && htmlViewMode === 'preview';
  if (isTextLikePayload(payload) && payload.renderMode === 'plainLargeText' && !htmlPreview) {
    return <TextViewerBody {...bodyProps} />;
  }

  switch (payload.kind) {
    case 'markdown':
      return <MarkdownViewerBody {...bodyProps} />;
    case 'code':
      // Code renders through PhoenixFileCodeView (Pierre), routed ahead of
      // renderBody in MetaViewer — this branch is unreachable.
      return null;
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
      // Plain text renders through PhoenixFileCodeView (Pierre), routed ahead of
      // renderBody in MetaViewer — this branch is unreachable.
      return null;
    case 'image':
      return <ImageViewerBody fileName={payload.fileName} url={payload.url} viewKey={imageViewKey} />;
  }
}
