/**
 * ProseReader Component (REQ-PF-005 through REQ-PF-013)
 *
 * File-content viewer with markdown/code rendering and long-press
 * annotation. Built on the shared `viewer/` primitives:
 *
 *   <ViewerShell>     // overlay/inline chrome, header, send button
 *     <FileBody />    // owned by this component — markdown / code / HTML
 *     <NotesPanel />  // shared review-notes side panel
 *     <AnnotationDialog />  // shared note-entry dialog
 *
 * Notes live in the conversation-scoped `ReviewNotesContext`, so
 * closing and reopening this viewer (or jumping to a different file
 * mid-review) preserves all pending notes until the user explicitly
 * sends or clears.
 */

import { useState, useEffect, useRef, useCallback, useMemo } from 'react';
import ReactMarkdown from 'react-markdown';
import type { Components } from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { SyntaxHighlighter, createElement, oneDark, oneLight } from '../utils/syntaxHighlighter';
import type { createElementProps } from '../utils/syntaxHighlighter';
import { useRegisterFocusScope } from '../hooks/useFocusScope';
import { useTheme } from '../hooks/useTheme';
import { useLongPress } from '../hooks/useLongPress';
import { useReviewNotes } from '../contexts/ReviewNotesContext';
import type { ReviewNote } from '../contexts/ReviewNotesContext';
import { ViewerShell } from './viewer/ViewerShell';
import { NotesPanel } from './viewer/NotesPanel';
import { AnnotationDialog } from './viewer/AnnotationDialog';
import { formatNotesForSend } from './viewer/formatNotes';
import { Loader2, AlertCircle, MessageSquarePlus } from 'lucide-react';

interface PatchContext {
  modifiedLines: Set<number>;
  firstModifiedLine?: number | undefined;
}

export interface ProseReaderProps {
  filePath: string;
  rootDir: string;
  onClose: () => void;
  onSendNotes: (notes: string) => void;
  patchContext?: PatchContext | undefined;
  /** Render inline (no overlay) for desktop split-pane mode (task 08654). */
  inline?: boolean;
}

// Re-exported for backward compatibility with external callers that
// imported the type from this module.
export type { ReviewNote } from '../contexts/ReviewNotesContext';

async function readFile(path: string): Promise<string> {
  const response = await fetch(`/api/files/read?path=${encodeURIComponent(path)}`);
  if (!response.ok) {
    const error = await response.json().catch(() => ({ error: 'Unknown error' }));
    throw new Error(error.error || 'Failed to read file');
  }
  const data = await response.json();
  return data.content;
}

function getFileType(path: string): 'markdown' | 'html' | 'code' | 'text' {
  const ext = path.split('.').pop()?.toLowerCase();
  if (!ext) return 'text';
  if (['md', 'markdown'].includes(ext)) return 'markdown';
  if (['html', 'htm'].includes(ext)) return 'html';
  if ([
    'rs', 'ts', 'tsx', 'js', 'jsx', 'py', 'go', 'java', 'cpp', 'c', 'h', 'hpp',
    'css', 'vue', 'svelte', 'php', 'rb', 'swift', 'kt', 'scala',
    'sh', 'bash', 'zsh', 'json', 'yaml', 'yml', 'toml', 'xml', 'sql', 'graphql',
  ].includes(ext)) return 'code';
  return 'text';
}

function getLanguage(path: string): string {
  const ext = path.split('.').pop()?.toLowerCase();
  const langMap: Record<string, string> = {
    rs: 'rust', ts: 'typescript', tsx: 'tsx', js: 'javascript', jsx: 'jsx',
    py: 'python', go: 'go', java: 'java', cpp: 'cpp', c: 'c', h: 'c', hpp: 'cpp',
    css: 'css', html: 'html', htm: 'html', vue: 'vue', svelte: 'svelte',
    php: 'php', rb: 'ruby', swift: 'swift', kt: 'kotlin', scala: 'scala',
    sh: 'bash', bash: 'bash', zsh: 'bash', json: 'json', yaml: 'yaml', yml: 'yaml',
    toml: 'toml', xml: 'xml', sql: 'sql', graphql: 'graphql', md: 'markdown',
  };
  return langMap[ext || ''] || 'text';
}

interface AnnotatableBlockProps {
  as?: React.ElementType;
  lineNumber: number;
  lineContent: string;
  onAnnotate: (lineNumber: number, lineContent: string) => void;
  isModified?: boolean;
  isHighlighted?: boolean;
  lineRef?: (el: HTMLElement | null) => void;
  className?: string;
  children?: React.ReactNode;
  [key: string]: unknown;
}

function AnnotatableBlock({
  as: Tag = 'div',
  lineNumber,
  lineContent,
  onAnnotate,
  isModified,
  isHighlighted,
  lineRef,
  className,
  children,
  ...rest
}: AnnotatableBlockProps) {
  const lp = useLongPress<{ lineNumber: number; lineContent: string }>(
    ({ lineNumber: ln, lineContent: lc }) => onAnnotate(ln, lc),
  );
  const cls = [
    'annotatable',
    className,
    isModified && 'annotatable--modified',
    isHighlighted && 'annotatable--highlighted',
  ].filter(Boolean).join(' ');

  return (
    <Tag
      ref={(el: HTMLElement | null) => lineRef?.(el)}
      className={cls}
      onTouchStart={(e: React.TouchEvent) => lp.start(e, { lineNumber, lineContent })}
      onTouchMove={lp.move}
      onTouchEnd={lp.end}
      onMouseDown={(e: React.MouseEvent) => lp.start(e, { lineNumber, lineContent })}
      onMouseMove={lp.move}
      onMouseUp={lp.end}
      onMouseLeave={lp.end}
      data-line={lineNumber}
      {...rest}
    >
      {children}
      <button
        className="annotatable__btn"
        onClick={(e: React.MouseEvent) => {
          e.stopPropagation();
          onAnnotate(lineNumber, lineContent);
        }}
        aria-label={`Add note to line ${lineNumber}`}
        title="Add note"
      >
        <MessageSquarePlus size={14} />
      </button>
    </Tag>
  );
}

export function ProseReader({
  filePath,
  rootDir,
  onClose,
  onSendNotes,
  patchContext,
  inline,
}: ProseReaderProps) {
  useRegisterFocusScope('prose-reader');
  const { theme } = useTheme();
  const syntaxStyle = theme === 'light' ? oneLight : oneDark;
  const reviewNotes = useReviewNotes();

  const [content, setContent] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [annotating, setAnnotating] = useState<{ lineNumber: number; lineContent: string } | null>(null);
  const [showPanel, setShowPanel] = useState(false);
  const [highlightedLine, setHighlightedLine] = useState<number | null>(null);
  const [htmlViewMode, setHtmlViewMode] = useState<'preview' | 'source'>('source');

  const lineRefs = useRef<Map<number, HTMLElement>>(new Map());
  const contentRef = useRef<HTMLDivElement>(null);

  const absolutePath = useMemo(() => {
    if (filePath.startsWith('/')) return filePath;
    return rootDir.endsWith('/') ? rootDir + filePath : rootDir + '/' + filePath;
  }, [filePath, rootDir]);

  const fileType = useMemo(() => getFileType(filePath), [filePath]);
  const language = useMemo(() => getLanguage(filePath), [filePath]);

  // Notes scoped to this file (for panel + per-line indicator).
  // Total pile (across all files + diff) drives the global send count.
  const fileNotes = useMemo(
    () => reviewNotes.notesForFile(absolutePath),
    [reviewNotes, absolutePath],
  );

  // Load file
  useEffect(() => {
    let cancelled = false;
    async function load() {
      setLoading(true);
      setError(null);
      try {
        const text = await readFile(absolutePath);
        if (!cancelled) setContent(text);
      } catch (err) {
        if (!cancelled) setError(err instanceof Error ? err.message : 'Failed to load file');
      } finally {
        if (!cancelled) setLoading(false);
      }
    }
    load();
    return () => { cancelled = true; };
  }, [absolutePath]);

  // Auto-scroll to first modified line (REQ-PF-014)
  useEffect(() => {
    if (content && patchContext?.firstModifiedLine) {
      const timer = setTimeout(() => {
        const lineEl = lineRefs.current.get(patchContext.firstModifiedLine!);
        if (lineEl) lineEl.scrollIntoView({ behavior: 'smooth', block: 'center' });
      }, 100);
      return () => clearTimeout(timer);
    }
    return undefined;
  }, [content, patchContext?.firstModifiedLine]);

  // Clear highlight after animation
  useEffect(() => {
    if (highlightedLine !== null) {
      const timer = setTimeout(() => setHighlightedLine(null), 2000);
      return () => clearTimeout(timer);
    }
    return undefined;
  }, [highlightedLine]);

  // Cmd/Ctrl+A: select all in viewer body. Guard against stealing the
  // shortcut from editable elements (annotation textarea, chat input,
  // any other input/textarea/contentEditable that happens to be open) —
  // those should keep their native "select all in field" behaviour.
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (!((e.ctrlKey || e.metaKey) && e.key === 'a')) return;
      const target = e.target as HTMLElement | null;
      if (target) {
        const tag = target.tagName;
        if (tag === 'INPUT' || tag === 'TEXTAREA' || target.isContentEditable) {
          return;
        }
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

  // Copy: collapse double newlines from block-element boundaries
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

  const handleAnnotate = useCallback((lineNumber: number, lineContent: string) => {
    setAnnotating({ lineNumber, lineContent });
  }, []);

  const handleSubmitNote = useCallback(
    (body: string) => {
      if (!annotating) return;
      const isModified = patchContext?.modifiedLines.has(annotating.lineNumber);
      const finalBody = isModified && !body.startsWith('[Changed line]')
        ? `[Changed line] ${body}`
        : body;
      reviewNotes.addNote(
        { kind: 'file', filePath: absolutePath, lineNumber: annotating.lineNumber },
        annotating.lineContent,
        finalBody,
      );
      setAnnotating(null);
    },
    [annotating, absolutePath, patchContext?.modifiedLines, reviewNotes],
  );

  const handleJumpTo = useCallback((note: ReviewNote) => {
    if (note.anchor.kind !== 'file' || note.anchor.filePath !== absolutePath) return;
    const el = lineRefs.current.get(note.anchor.lineNumber);
    if (el) {
      el.scrollIntoView({ behavior: 'smooth', block: 'center' });
      setHighlightedLine(note.anchor.lineNumber);
    }
    setShowPanel(false);
  }, [absolutePath]);

  const handleSend = useCallback(() => {
    const formatted = formatNotesForSend(reviewNotes.notes);
    if (formatted) {
      onSendNotes(formatted);
      reviewNotes.clear();
      setShowPanel(false);
    }
  }, [reviewNotes, onSendNotes]);

  const renderLines = useMemo(() => {
    if (!content) return null;
    const lines = content.split('\n');
    return lines.map((line, index) => {
      const lineNumber = index + 1;
      return (
        <AnnotatableBlock
          key={lineNumber}
          lineNumber={lineNumber}
          lineContent={line}
          onAnnotate={handleAnnotate}
          className="prose-line"
          isModified={patchContext?.modifiedLines.has(lineNumber) ?? false}
          isHighlighted={highlightedLine === lineNumber}
          lineRef={(el) => {
            if (el) lineRefs.current.set(lineNumber, el);
            else lineRefs.current.delete(lineNumber);
          }}
        >
          <span className="prose-line__number">{lineNumber}</span>
          <span className="prose-line__content">{line || ' '}</span>
        </AnnotatableBlock>
      );
    });
  }, [content, patchContext?.modifiedLines, highlightedLine, handleAnnotate]);

  const renderMarkdown = useMemo(() => {
    if (!content || fileType !== 'markdown') return null;
    const rawLines = content.split('\n');

    const annotatable = (Tag: React.ElementType) =>
      ({ children, node, ...props }: { children?: React.ReactNode; node?: { position?: { start?: { line?: number }; end?: { line?: number } } }; [key: string]: unknown }) => {
        const ln = node?.position?.start?.line ?? 0;
        const startLine = (node?.position?.start?.line ?? 1) - 1;
        const endLine = (node?.position?.end?.line ?? startLine + 1) - 1;
        const rawLineContent = rawLines.slice(startLine, endLine + 1).join(' ').slice(0, 200);
        return (
          <AnnotatableBlock
            as={Tag}
            lineNumber={ln}
            lineContent={rawLineContent}
            onAnnotate={handleAnnotate}
            className="prose-block"
            isModified={patchContext?.modifiedLines.has(ln) ?? false}
            isHighlighted={highlightedLine === ln}
            lineRef={(el) => { if (el) lineRefs.current.set(ln, el); }}
            {...props}
          >
            {children}
          </AnnotatableBlock>
        );
      };

    return (
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          p: annotatable('p'),
          h1: annotatable('h1'),
          h2: annotatable('h2'),
          h3: annotatable('h3'),
          td: annotatable('td'),
          th: annotatable('th'),
          li: annotatable('li'),
          blockquote: annotatable('blockquote'),
          code: ({ inline, className, children, ...props }: { inline?: boolean; className?: string; children?: React.ReactNode; [key: string]: unknown }) => {
            const match = /language-(\w+)/.exec(className || '');
            return !inline && match ? (
              <SyntaxHighlighter style={syntaxStyle} language={match[1]} PreTag="div" {...props}>
                {String(children).replace(/\n$/, '')}
              </SyntaxHighlighter>
            ) : (
              <code className={className} {...props}>{children}</code>
            );
          },
        } as unknown as Components}
      >
        {content}
      </ReactMarkdown>
    );
  }, [content, fileType, patchContext?.modifiedLines, highlightedLine, handleAnnotate, syntaxStyle]);

  const fileName = filePath.split('/').pop() || filePath;
  // Panel + badge scope = THIS file's notes. Cross-viewer notes
  // (other files, the diff) live in the same global pile and surface
  // in their own viewer's panel; Send All still drops the entire pile
  // so the user doesn't lose them. Per Copilot review feedback (2026-05-01):
  // the panel previously showed all notes but `handleJumpTo` only worked
  // for this-file anchors, so cross-viewer entries were no-op clicks.

  const headerExtras = fileType === 'html' ? (
    <>
      <button
        className={`viewer-shell-toggle ${htmlViewMode === 'preview' ? 'active' : ''}`}
        onClick={() => setHtmlViewMode(htmlViewMode === 'preview' ? 'source' : 'preview')}
        title={htmlViewMode === 'preview' ? 'Show source' : 'Show sandboxed preview (no scripts)'}
      >
        {htmlViewMode === 'preview' ? '</>' : 'Preview'}
      </button>
      <a
        className="viewer-shell-toggle"
        href={`/preview${absolutePath}`}
        target="_blank"
        rel="noopener noreferrer"
        title="Open in new tab (full render with scripts)"
      >
        Open in browser
      </a>
    </>
  ) : null;

  return (
    <ViewerShell
      mode={inline ? 'inline' : 'overlay'}
      ariaLabel={`File viewer: ${fileName}`}
      title={fileName}
      titleTooltip={absolutePath}
      headerExtras={headerExtras}
      noteCount={fileNotes.length}
      onToggleNotes={() => setShowPanel((v) => !v)}
      onSend={handleSend}
      banner={
        patchContext && patchContext.modifiedLines.size > 0 ? (
          <span>
            Viewing {fileName}: {patchContext.modifiedLines.size} change
            {patchContext.modifiedLines.size !== 1 ? 's' : ''} from patch
          </span>
        ) : null
      }
      onClose={onClose}
      panel={
        showPanel ? (
          <NotesPanel
            notes={fileNotes}
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
            anchorLabel={`Line ${annotating.lineNumber}`}
            lineContent={annotating.lineContent}
            onSubmit={handleSubmitNote}
            onCancel={() => setAnnotating(null)}
          />
        ) : null
      }
    >
      <div className="prose-reader-content" ref={contentRef}>
        {loading ? (
          <div className="prose-reader-loading">
            <Loader2 size={32} className="spinning" />
            <span>Loading file...</span>
          </div>
        ) : error ? (
          <div className="prose-reader-error">
            <AlertCircle size={32} />
            <span>{error}</span>
            <button onClick={onClose}>Close</button>
          </div>
        ) : fileType === 'markdown' ? (
          <div className="prose-reader-markdown">{renderMarkdown}</div>
        ) : fileType === 'html' && htmlViewMode === 'preview' ? (
          <div className="prose-reader-html-preview">
            <iframe
              src={`/preview${absolutePath}`}
              sandbox="allow-same-origin"
              title="HTML Preview"
              className="prose-reader-iframe"
            />
          </div>
        ) : (fileType === 'html' && htmlViewMode === 'source') || fileType === 'code' ? (
          <div className="prose-reader-code">
            <SyntaxHighlighter
              style={oneDark}
              language={language}
              showLineNumbers
              renderer={({ rows, stylesheet, useInlineStyles }: { rows: unknown[]; stylesheet: unknown; useInlineStyles: boolean }) => {
                const lines = content?.split('\n') || [];
                return (
                  <>
                    {rows.map((node, idx) => {
                      const lineNumber = idx + 1;
                      return (
                        <AnnotatableBlock
                          key={lineNumber}
                          as="div"
                          lineNumber={lineNumber}
                          lineContent={lines[idx] || ''}
                          onAnnotate={handleAnnotate}
                          className="prose-code-line"
                          isModified={patchContext?.modifiedLines.has(lineNumber) ?? false}
                          isHighlighted={highlightedLine === lineNumber}
                          lineRef={(el) => { if (el) lineRefs.current.set(lineNumber, el); }}
                        >
                          {createElement({ node, stylesheet, useInlineStyles, key: `t-${idx}` } as createElementProps)}
                        </AnnotatableBlock>
                      );
                    })}
                  </>
                );
              }}
            >
              {content || ''}
            </SyntaxHighlighter>
          </div>
        ) : (
          <div className="prose-reader-text">{renderLines}</div>
        )}
      </div>
      {/* Per-line indicator: dots in the gutter where notes exist (future). */}
      {fileNotes.length > 0 && null}
    </ViewerShell>
  );
}
