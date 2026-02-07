/**
 * ProseReader Component
 * 
 * Implements REQ-PF-005 through REQ-PF-013:
 * - File content display with markdown/code rendering
 * - Long-press gesture for annotation (with 10px threshold)
 * - Notes management and formatting
 * - Patch integration with diff highlighting
 */

import { useState, useEffect, useRef, useCallback, useMemo } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { Prism as SyntaxHighlighter } from 'react-syntax-highlighter';
import { oneDark } from 'react-syntax-highlighter/dist/esm/styles/prism';
import {
  ArrowLeft,
  X,
  Loader2,
  AlertCircle,
  MessageSquare,
  Trash2,
  Send,
  ChevronDown,
} from 'lucide-react';

// Types
export interface ReviewNote {
  id: string;
  filePath: string;
  lineNumber: number;
  lineContent: string;
  note: string;
  timestamp: number;
}

interface PatchContext {
  modifiedLines: Set<number>;
  firstModifiedLine?: number;
}

interface ProseReaderProps {
  filePath: string;
  rootDir: string;
  onClose: () => void;
  onSendNotes: (notes: string) => void;
  patchContext?: PatchContext;
}

// API function
async function readFile(path: string): Promise<string> {
  const response = await fetch(`/api/files/read?path=${encodeURIComponent(path)}`);
  if (!response.ok) {
    const error = await response.json().catch(() => ({ error: 'Unknown error' }));
    throw new Error(error.error || 'Failed to read file');
  }
  const data = await response.json();
  return data.content;
}

// Detect file type from extension
function getFileType(path: string): 'markdown' | 'code' | 'text' {
  const ext = path.split('.').pop()?.toLowerCase();
  if (!ext) return 'text';

  if (['md', 'markdown'].includes(ext)) return 'markdown';
  if ([
    'rs', 'ts', 'tsx', 'js', 'jsx', 'py', 'go', 'java', 'cpp', 'c', 'h', 'hpp',
    'css', 'html', 'htm', 'vue', 'svelte', 'php', 'rb', 'swift', 'kt', 'scala',
    'sh', 'bash', 'zsh', 'json', 'yaml', 'yml', 'toml', 'xml', 'sql', 'graphql'
  ].includes(ext)) return 'code';

  return 'text';
}

// Get language for syntax highlighting
function getLanguage(path: string): string {
  const ext = path.split('.').pop()?.toLowerCase();
  const langMap: Record<string, string> = {
    rs: 'rust', ts: 'typescript', tsx: 'tsx', js: 'javascript', jsx: 'jsx',
    py: 'python', go: 'go', java: 'java', cpp: 'cpp', c: 'c', h: 'c', hpp: 'cpp',
    css: 'css', html: 'html', htm: 'html', vue: 'vue', svelte: 'svelte',
    php: 'php', rb: 'ruby', swift: 'swift', kt: 'kotlin', scala: 'scala',
    sh: 'bash', bash: 'bash', zsh: 'bash', json: 'json', yaml: 'yaml', yml: 'yaml',
    toml: 'toml', xml: 'xml', sql: 'sql', graphql: 'graphql', md: 'markdown'
  };
  return langMap[ext || ''] || 'text';
}

// Long-press hook
function useLongPress(
  onLongPress: (lineNumber: number, lineContent: string) => void,
  threshold = 500,
  movementThreshold = 10
) {
  const timerRef = useRef<number | null>(null);
  const startPosRef = useRef<{ x: number; y: number } | null>(null);
  const lineDataRef = useRef<{ lineNumber: number; lineContent: string } | null>(null);

  const cancel = useCallback(() => {
    if (timerRef.current) {
      window.clearTimeout(timerRef.current);
      timerRef.current = null;
    }
    startPosRef.current = null;
    lineDataRef.current = null;
  }, []);

  const start = useCallback((e: React.TouchEvent | React.MouseEvent, lineNumber: number, lineContent: string) => {
    const pos = 'touches' in e 
      ? { x: e.touches[0].clientX, y: e.touches[0].clientY }
      : { x: e.clientX, y: e.clientY };
    
    startPosRef.current = pos;
    lineDataRef.current = { lineNumber, lineContent };

    timerRef.current = window.setTimeout(() => {
      // Trigger haptic feedback if available
      if ('vibrate' in navigator) {
        navigator.vibrate(50);
      }
      onLongPress(lineNumber, lineContent);
      cancel();
    }, threshold);
  }, [onLongPress, threshold, cancel]);

  const move = useCallback((e: React.TouchEvent | React.MouseEvent) => {
    if (!startPosRef.current) return;

    const pos = 'touches' in e
      ? { x: e.touches[0].clientX, y: e.touches[0].clientY }
      : { x: e.clientX, y: e.clientY };

    const deltaX = Math.abs(pos.x - startPosRef.current.x);
    const deltaY = Math.abs(pos.y - startPosRef.current.y);

    // Cancel if moved more than threshold (REQ-PF-006: 10px threshold)
    if (deltaX > movementThreshold || deltaY > movementThreshold) {
      cancel();
    }
  }, [movementThreshold, cancel]);

  const end = useCallback(() => {
    cancel();
  }, [cancel]);

  return { start, move, end };
}

// Line wrapper component for gesture handling
function SelectableLine({
  lineNumber,
  content,
  isModified,
  isHighlighted,
  onLongPress,
  lineRef,
}: {
  lineNumber: number;
  content: string;
  isModified?: boolean;
  isHighlighted?: boolean;
  onLongPress: (lineNumber: number, lineContent: string) => void;
  lineRef?: (el: HTMLDivElement | null) => void;
}) {
  const { start, move, end } = useLongPress(onLongPress);

  return (
    <div
      ref={lineRef}
      className={`prose-reader-line ${isModified ? 'prose-reader-line--modified' : ''} ${isHighlighted ? 'prose-reader-line--highlighted' : ''}`}
      onTouchStart={(e) => start(e, lineNumber, content)}
      onTouchMove={move}
      onTouchEnd={end}
      onMouseDown={(e) => start(e, lineNumber, content)}
      onMouseMove={move}
      onMouseUp={end}
      onMouseLeave={end}
      data-line={lineNumber}
    >
      <span className="prose-reader-line-number">{lineNumber}</span>
      <span className="prose-reader-line-content">{content || '\u00A0'}</span>
    </div>
  );
}

export function ProseReader({
  filePath,
  rootDir,
  onClose,
  onSendNotes,
  patchContext,
}: ProseReaderProps) {
  const [content, setContent] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [notes, setNotes] = useState<ReviewNote[]>([]);
  const [annotatingLine, setAnnotatingLine] = useState<{ lineNumber: number; lineContent: string } | null>(null);
  const [noteInput, setNoteInput] = useState('');
  const [showNotesPanel, setShowNotesPanel] = useState(false);
  const [highlightedLine, setHighlightedLine] = useState<number | null>(null);
  const [showCloseConfirm, setShowCloseConfirm] = useState(false);

  const noteInputRef = useRef<HTMLTextAreaElement>(null);
  const lineRefs = useRef<Map<number, HTMLDivElement>>(new Map());
  const contentRef = useRef<HTMLDivElement>(null);

  // Compute absolute path
  const absolutePath = useMemo(() => {
    if (filePath.startsWith('/')) return filePath;
    return rootDir.endsWith('/') ? rootDir + filePath : rootDir + '/' + filePath;
  }, [filePath, rootDir]);

  // File type for rendering
  const fileType = useMemo(() => getFileType(filePath), [filePath]);
  const language = useMemo(() => getLanguage(filePath), [filePath]);

  // Load file content
  useEffect(() => {
    let cancelled = false;

    async function load() {
      setLoading(true);
      setError(null);
      try {
        const text = await readFile(absolutePath);
        if (!cancelled) {
          setContent(text);
        }
      } catch (err) {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : 'Failed to load file');
        }
      } finally {
        if (!cancelled) {
          setLoading(false);
        }
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
        if (lineEl) {
          lineEl.scrollIntoView({ behavior: 'smooth', block: 'center' });
        }
      }, 100);
      return () => clearTimeout(timer);
    }
  }, [content, patchContext?.firstModifiedLine]);

  // Focus note input when dialog opens
  useEffect(() => {
    if (annotatingLine && noteInputRef.current) {
      noteInputRef.current.focus();
    }
  }, [annotatingLine]);

  // Clear highlight after animation
  useEffect(() => {
    if (highlightedLine !== null) {
      const timer = setTimeout(() => setHighlightedLine(null), 2000);
      return () => clearTimeout(timer);
    }
  }, [highlightedLine]);

  // Handle long press to annotate
  const handleLongPress = useCallback((lineNumber: number, lineContent: string) => {
    setAnnotatingLine({ lineNumber, lineContent });
    setNoteInput('');
  }, []);

  // Add a note
  const handleAddNote = useCallback(() => {
    if (!annotatingLine || !noteInput.trim()) return;

    const isModified = patchContext?.modifiedLines.has(annotatingLine.lineNumber);
    let finalNote = noteInput.trim();
    
    // Auto-prefix for modified lines (REQ-PF-014)
    if (isModified && !finalNote.startsWith('[Changed line]')) {
      finalNote = `[Changed line] ${finalNote}`;
    }

    const note: ReviewNote = {
      id: crypto.randomUUID(),
      filePath: absolutePath,
      lineNumber: annotatingLine.lineNumber,
      lineContent: annotatingLine.lineContent,
      note: finalNote,
      timestamp: Date.now(),
    };

    setNotes(prev => [...prev, note]);
    setAnnotatingLine(null);
    setNoteInput('');
  }, [annotatingLine, noteInput, absolutePath, patchContext?.modifiedLines]);

  // Delete a note
  const handleDeleteNote = useCallback((id: string) => {
    setNotes(prev => prev.filter(n => n.id !== id));
  }, []);

  // Clear all notes
  const handleClearAll = useCallback(() => {
    setNotes([]);
    setShowNotesPanel(false);
  }, []);

  // Jump to line
  const handleJumpToLine = useCallback((lineNumber: number) => {
    const lineEl = lineRefs.current.get(lineNumber);
    if (lineEl) {
      lineEl.scrollIntoView({ behavior: 'smooth', block: 'center' });
      setHighlightedLine(lineNumber);
    }
    setShowNotesPanel(false);
  }, []);

  // Format and send notes (REQ-PF-009)
  const handleSendNotes = useCallback(() => {
    if (notes.length === 0) return;

    const formatted = `Review notes for \`${absolutePath}\`:\n\n` +
      notes.map(n => 
        `> Line ${n.lineNumber}: \`${n.lineContent}\`\n${n.note}`
      ).join('\n\n');

    onSendNotes(formatted);
    setNotes([]);
    setShowNotesPanel(false);
  }, [notes, absolutePath, onSendNotes]);

  // Handle close with unsaved notes warning (REQ-PF-010)
  const handleClose = useCallback(() => {
    if (notes.length > 0) {
      setShowCloseConfirm(true);
    } else {
      onClose();
    }
  }, [notes.length, onClose]);

  // Keyboard shortcuts
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (annotatingLine) {
        if (e.key === 'Escape') {
          setAnnotatingLine(null);
        } else if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') {
          handleAddNote();
        }
      } else if (showCloseConfirm) {
        if (e.key === 'Escape') {
          setShowCloseConfirm(false);
        }
      } else {
        if (e.key === 'Escape') {
          handleClose();
        }
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [annotatingLine, showCloseConfirm, handleAddNote, handleClose]);

  // Render lines for code/text files
  const renderLines = useMemo(() => {
    if (!content) return null;
    const lines = content.split('\n');

    return lines.map((line, index) => {
      const lineNumber = index + 1;
      const isModified = patchContext?.modifiedLines.has(lineNumber);
      const isHighlightedLine = highlightedLine === lineNumber;

      return (
        <SelectableLine
          key={lineNumber}
          lineNumber={lineNumber}
          content={line}
          isModified={isModified}
          isHighlighted={isHighlightedLine}
          onLongPress={handleLongPress}
          lineRef={(el) => {
            if (el) lineRefs.current.set(lineNumber, el);
            else lineRefs.current.delete(lineNumber);
          }}
        />
      );
    });
  }, [content, patchContext?.modifiedLines, highlightedLine, handleLongPress]);

  // Render markdown with selectable paragraphs
  const renderMarkdown = useMemo(() => {
    if (!content || fileType !== 'markdown') return null;

    // Track line numbers for markdown blocks
    let lineCounter = 0;

    return (
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          // Wrap each block element with long-press handlers
          p: ({ children, ...props }) => {
            lineCounter++;
            const lineNumber = lineCounter;
            const lineContent = String(children).slice(0, 200);
            const { start, move, end } = useLongPress(handleLongPress);
            const isModified = patchContext?.modifiedLines.has(lineNumber);
            const isHighlightedLine = highlightedLine === lineNumber;

            return (
              <p
                {...props}
                className={`prose-reader-block ${isModified ? 'prose-reader-line--modified' : ''} ${isHighlightedLine ? 'prose-reader-line--highlighted' : ''}`}
                onTouchStart={(e) => start(e, lineNumber, lineContent)}
                onTouchMove={move}
                onTouchEnd={end}
                onMouseDown={(e) => start(e, lineNumber, lineContent)}
                onMouseMove={move}
                onMouseUp={end}
                onMouseLeave={end}
                ref={(el) => {
                  if (el) lineRefs.current.set(lineNumber, el);
                }}
              >
                {children}
              </p>
            );
          },
          h1: ({ children, ...props }) => {
            lineCounter++;
            const lineNumber = lineCounter;
            const lineContent = String(children);
            const { start, move, end } = useLongPress(handleLongPress);

            return (
              <h1
                {...props}
                className="prose-reader-block"
                onTouchStart={(e) => start(e, lineNumber, lineContent)}
                onTouchMove={move}
                onTouchEnd={end}
                onMouseDown={(e) => start(e, lineNumber, lineContent)}
                onMouseMove={move}
                onMouseUp={end}
                onMouseLeave={end}
              >
                {children}
              </h1>
            );
          },
          h2: ({ children, ...props }) => {
            lineCounter++;
            const lineNumber = lineCounter;
            const lineContent = String(children);
            const { start, move, end } = useLongPress(handleLongPress);

            return (
              <h2
                {...props}
                className="prose-reader-block"
                onTouchStart={(e) => start(e, lineNumber, lineContent)}
                onTouchMove={move}
                onTouchEnd={end}
                onMouseDown={(e) => start(e, lineNumber, lineContent)}
                onMouseMove={move}
                onMouseUp={end}
                onMouseLeave={end}
              >
                {children}
              </h2>
            );
          },
          h3: ({ children, ...props }) => {
            lineCounter++;
            const lineNumber = lineCounter;
            const lineContent = String(children);
            const { start, move, end } = useLongPress(handleLongPress);

            return (
              <h3
                {...props}
                className="prose-reader-block"
                onTouchStart={(e) => start(e, lineNumber, lineContent)}
                onTouchMove={move}
                onTouchEnd={end}
                onMouseDown={(e) => start(e, lineNumber, lineContent)}
                onMouseMove={move}
                onMouseUp={end}
                onMouseLeave={end}
              >
                {children}
              </h3>
            );
          },
          code: ({ inline, className, children, ...props }: any) => {
            const match = /language-(\w+)/.exec(className || '');
            return !inline && match ? (
              <SyntaxHighlighter
                style={oneDark}
                language={match[1]}
                PreTag="div"
                {...props}
              >
                {String(children).replace(/\n$/, '')}
              </SyntaxHighlighter>
            ) : (
              <code className={className} {...props}>
                {children}
              </code>
            );
          },
        }}
      >
        {content}
      </ReactMarkdown>
    );
  }, [content, fileType, patchContext?.modifiedLines, highlightedLine, handleLongPress]);

  // Get file name from path
  const fileName = filePath.split('/').pop() || filePath;

  return (
    <div className="prose-reader-overlay">
      {/* Header */}
      <div className="prose-reader-header">
        <button
          className="prose-reader-btn"
          onClick={handleClose}
          aria-label="Close reader"
        >
          <ArrowLeft size={20} />
        </button>
        <div className="prose-reader-title" title={absolutePath}>
          {fileName}
        </div>
        <div className="prose-reader-actions">
          {notes.length > 0 && (
            <>
              <button
                className="prose-reader-badge"
                onClick={() => setShowNotesPanel(!showNotesPanel)}
                aria-label={`${notes.length} notes`}
              >
                <MessageSquare size={18} />
                <span>{notes.length}</span>
              </button>
              <button
                className="prose-reader-send-btn"
                onClick={handleSendNotes}
                aria-label="Send notes"
              >
                <Send size={18} />
              </button>
            </>
          )}
        </div>
      </div>

      {/* Patch context banner (REQ-PF-014) */}
      {patchContext && (
        <div className="prose-reader-banner">
          <span className="prose-reader-banner-text">
            Viewing {fileName}: {patchContext.modifiedLines.size} change{patchContext.modifiedLines.size !== 1 ? 's' : ''} from patch
          </span>
        </div>
      )}

      {/* Content */}
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
            <button onClick={handleClose}>Close</button>
          </div>
        ) : fileType === 'markdown' ? (
          <div className="prose-reader-markdown">
            {renderMarkdown}
          </div>
        ) : fileType === 'code' ? (
          <div className="prose-reader-code">
            <SyntaxHighlighter
              style={oneDark}
              language={language}
              showLineNumbers
              wrapLines
              lineProps={(lineNumber) => ({
                style: {
                  display: 'block',
                  backgroundColor: patchContext?.modifiedLines.has(lineNumber)
                    ? 'rgba(255, 236, 156, 0.3)'
                    : highlightedLine === lineNumber
                    ? 'rgba(59, 130, 246, 0.3)'
                    : undefined,
                  borderLeft: patchContext?.modifiedLines.has(lineNumber)
                    ? '3px solid #f0ad4e'
                    : undefined,
                },
                onTouchStart: (e: React.TouchEvent) => {
                  const { start } = useLongPress(handleLongPress);
                  const lineContent = (content?.split('\n')[lineNumber - 1] || '');
                  start(e, lineNumber, lineContent);
                },
                onMouseDown: (e: React.MouseEvent) => {
                  const { start } = useLongPress(handleLongPress);
                  const lineContent = (content?.split('\n')[lineNumber - 1] || '');
                  start(e, lineNumber, lineContent);
                },
                ref: (el: HTMLDivElement | null) => {
                  if (el) lineRefs.current.set(lineNumber, el);
                },
              })}
            >
              {content || ''}
            </SyntaxHighlighter>
          </div>
        ) : (
          <div className="prose-reader-text">
            {renderLines}
          </div>
        )}
      </div>

      {/* Annotation Dialog (REQ-PF-007) */}
      {annotatingLine && (
        <div className="prose-reader-annotation-overlay" onClick={() => setAnnotatingLine(null)}>
          <div className="prose-reader-annotation-dialog" onClick={(e) => e.stopPropagation()}>
            <div className="prose-reader-annotation-header">
              <span>Line {annotatingLine.lineNumber}</span>
              <button onClick={() => setAnnotatingLine(null)}>
                <X size={18} />
              </button>
            </div>
            <div className="prose-reader-annotation-preview">
              {annotatingLine.lineContent.slice(0, 100)}
              {annotatingLine.lineContent.length > 100 && '...'}
            </div>
            <textarea
              ref={noteInputRef}
              className="prose-reader-annotation-input"
              placeholder="Add your note..."
              value={noteInput}
              onChange={(e) => setNoteInput(e.target.value)}
              rows={3}
            />
            <div className="prose-reader-annotation-actions">
              <button onClick={() => setAnnotatingLine(null)}>Cancel</button>
              <button
                className="primary"
                onClick={handleAddNote}
                disabled={!noteInput.trim()}
              >
                Add Note
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Notes Panel (REQ-PF-008) */}
      {showNotesPanel && (
        <div className="prose-reader-notes-panel">
          <div className="prose-reader-notes-header">
            <span>Notes ({notes.length})</span>
            <button onClick={() => setShowNotesPanel(false)}>
              <ChevronDown size={18} />
            </button>
          </div>
          <div className="prose-reader-notes-list">
            {notes.map((note) => (
              <div key={note.id} className="prose-reader-note">
                <div className="prose-reader-note-header">
                  <button
                    className="prose-reader-note-line"
                    onClick={() => handleJumpToLine(note.lineNumber)}
                  >
                    Line {note.lineNumber}
                  </button>
                  <button
                    className="prose-reader-note-delete"
                    onClick={() => handleDeleteNote(note.id)}
                  >
                    <Trash2 size={14} />
                  </button>
                </div>
                <div className="prose-reader-note-preview">
                  {note.lineContent.slice(0, 60)}
                  {note.lineContent.length > 60 && '...'}
                </div>
                <div className="prose-reader-note-text">{note.note}</div>
              </div>
            ))}
          </div>
          <div className="prose-reader-notes-actions">
            <button onClick={handleClearAll}>Clear All</button>
            <button className="primary" onClick={handleSendNotes}>
              <Send size={16} />
              Send All
            </button>
          </div>
        </div>
      )}

      {/* Close Confirmation (REQ-PF-010) */}
      {showCloseConfirm && (
        <div className="prose-reader-confirm-overlay" onClick={() => setShowCloseConfirm(false)}>
          <div className="prose-reader-confirm-dialog" onClick={(e) => e.stopPropagation()}>
            <p>You have {notes.length} unsaved note{notes.length !== 1 ? 's' : ''}. Discard them?</p>
            <div className="prose-reader-confirm-actions">
              <button onClick={() => setShowCloseConfirm(false)}>Cancel</button>
              <button
                className="danger"
                onClick={() => {
                  setNotes([]);
                  onClose();
                }}
              >
                Discard
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
