/**
 * TaskApprovalReader Component
 *
 * Renders a task for approval. The user MUST choose one of:
 * Approve, Discard, or Send Feedback. The overlay cannot be dismissed
 * by Escape, back button, or clicking outside.
 *
 * Annotations use the same long-press idiom as the file viewer, but this
 * component is intentionally separate from the MetaViewer file-review stack
 * (local approval-feedback notes, not the conversation ReviewNotesContext; a
 * non-dismissible phase overlay, not a viewer slot). See specs/prose-feedback/
 * for the rationale. Plan content comes from ConversationState, not from disk.
 */

import { useState, useEffect, useRef, useCallback, useMemo } from 'react';
import ReactMarkdown from 'react-markdown';
import type { Components } from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { SyntaxHighlighter, oneDark } from '../utils/syntaxHighlighter';
import { MermaidDiagram } from './MermaidDiagram';
import { generateUUID } from '../utils/uuid';
import type { TaskApprovalHandoff } from '../api';
import { useRegisterFocusScope } from '../hooks/useFocusScope';
import {
  X,
  MessageSquare,
  MessageSquarePlus,
  Trash2,
  Send,
  ChevronDown,
  Check,
  Loader2,
} from 'lucide-react';

// Reuse ReviewNote type shape
interface ReviewNote {
  id: string;
  lineNumber: number;
  lineContent: string;
  note: string;
  timestamp: number;
}

const formatTaskApprovalContextPercent = (used: number, max: number): string => {
  if (max <= 0) return '0%';
  const percent = Math.min(Math.max((used / max) * 100, 0), 100);
  return `${Math.round(percent)}%`;
};

export interface TaskApprovalReaderProps {
  title: string;
  priority: string;
  plan: string;
  contextWindowUsed?: number;
  modelContextWindow?: number;
  onApprove: (handoff: TaskApprovalHandoff) => void;
  onReject: () => void;
  onSendFeedback: (annotations: string) => void;
}

// Long-press hook (same as ProseReader)
function useLongPress(
  onLongPress: (lineNumber: number, lineContent: string) => void,
  threshold = 500,
  movementThreshold = 10
) {
  const timerRef = useRef<number | null>(null);
  const startPosRef = useRef<{ x: number; y: number } | null>(null);

  const cancel = useCallback(() => {
    if (timerRef.current) {
      window.clearTimeout(timerRef.current);
      timerRef.current = null;
    }
    startPosRef.current = null;
  }, []);

  const start = useCallback(
    (
      e: React.TouchEvent | React.MouseEvent,
      lineNumber: number,
      lineContent: string
    ) => {
      const touch = 'touches' in e ? e.touches[0] : undefined;
      const pos = touch
        ? { x: touch.clientX, y: touch.clientY }
        : { x: (e as React.MouseEvent).clientX, y: (e as React.MouseEvent).clientY };

      startPosRef.current = pos;

      timerRef.current = window.setTimeout(() => {
        if ('vibrate' in navigator) {
          navigator.vibrate(50);
        }
        onLongPress(lineNumber, lineContent);
        cancel();
      }, threshold);
    },
    [onLongPress, threshold, cancel]
  );

  const move = useCallback(
    (e: React.TouchEvent | React.MouseEvent) => {
      if (!startPosRef.current) return;

      const touch = 'touches' in e ? e.touches[0] : undefined;
      const pos = touch
        ? { x: touch.clientX, y: touch.clientY }
        : { x: (e as React.MouseEvent).clientX, y: (e as React.MouseEvent).clientY };

      const deltaX = Math.abs(pos.x - startPosRef.current.x);
      const deltaY = Math.abs(pos.y - startPosRef.current.y);

      if (deltaX > movementThreshold || deltaY > movementThreshold) {
        cancel();
      }
    },
    [movementThreshold, cancel]
  );

  const end = useCallback(() => {
    cancel();
  }, [cancel]);

  return { start, move, end };
}

// Annotatable block wrapper
interface AnnotatableBlockProps {
  as?: React.ElementType;
  lineNumber: number;
  lineContent: string;
  onAnnotate: (lineNumber: number, lineContent: string) => void;
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
  isHighlighted,
  lineRef,
  className,
  children,
  ...rest
}: AnnotatableBlockProps) {
  const { start, move, end } = useLongPress(onAnnotate);
  const cls = [
    'annotatable',
    className,
    isHighlighted && 'annotatable--highlighted',
  ]
    .filter(Boolean)
    .join(' ');

  return (
    <Tag
      ref={(el: HTMLElement | null) => lineRef?.(el)}
      className={cls}
      onTouchStart={(e: React.TouchEvent) => start(e, lineNumber, lineContent)}
      onTouchMove={move}
      onTouchEnd={end}
      onMouseDown={(e: React.MouseEvent) => start(e, lineNumber, lineContent)}
      onMouseMove={move}
      onMouseUp={end}
      onMouseLeave={end}
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

export function TaskApprovalReader({
  title,
  priority,
  plan,
  contextWindowUsed,
  modelContextWindow,
  onApprove,
  onReject,
  onSendFeedback,
}: TaskApprovalReaderProps) {
  useRegisterFocusScope('task-approval');

  const [approvingHandoff, setApprovingHandoff] = useState<TaskApprovalHandoff | null>(null);
  const [notes, setNotes] = useState<ReviewNote[]>([]);
  const [annotatingLine, setAnnotatingLine] = useState<{
    lineNumber: number;
    lineContent: string;
  } | null>(null);
  const [noteInput, setNoteInput] = useState('');
  const [showNotesPanel, setShowNotesPanel] = useState(false);
  const [highlightedLine, setHighlightedLine] = useState<number | null>(null);
  const [discardConfirmOpen, setDiscardConfirmOpen] = useState(false);
  const hasUnsentNotes = notes.length > 0;
  const noteCountLabel = `${notes.length} note${notes.length !== 1 ? 's' : ''}`;

  const contextUsagePercent =
    contextWindowUsed !== undefined && modelContextWindow !== undefined
      ? formatTaskApprovalContextPercent(contextWindowUsed, modelContextWindow)
      : null;

  const noteInputRef = useRef<HTMLTextAreaElement>(null);
  const lineRefs = useRef<Map<number, HTMLElement>>(new Map());

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
    return undefined;
  }, [highlightedLine]);

  const handleAddNote = useCallback(() => {
    if (!annotatingLine || !noteInput.trim()) return;

    const note: ReviewNote = {
      id: generateUUID(),
      lineNumber: annotatingLine.lineNumber,
      lineContent: annotatingLine.lineContent,
      note: noteInput.trim(),
      timestamp: Date.now(),
    };

    setNotes((prev) => [...prev, note]);
    setAnnotatingLine(null);
    setNoteInput('');
  }, [annotatingLine, noteInput]);

  // Block Escape from closing — only allow it to dismiss annotation dialog or discard confirm
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        if (annotatingLine) {
          setAnnotatingLine(null);
        } else if (discardConfirmOpen) {
          setDiscardConfirmOpen(false);
        }
        // Otherwise: do nothing. Cannot close the approval reader via Escape.
      }
      if (annotatingLine && (e.ctrlKey || e.metaKey) && e.key === 'Enter') {
        handleAddNote();
      }
    };

    window.addEventListener('keydown', handleKeyDown, true); // capture phase
    return () => window.removeEventListener('keydown', handleKeyDown, true);
  }, [annotatingLine, discardConfirmOpen, handleAddNote]);

  const handleLongPress = useCallback(
    (lineNumber: number, lineContent: string) => {
      setAnnotatingLine({ lineNumber, lineContent });
      setNoteInput('');
    },
    []
  );

  const handleDeleteNote = useCallback((id: string) => {
    setNotes((prev) => prev.filter((n) => n.id !== id));
  }, []);

  const handleClearAll = useCallback(() => {
    setNotes([]);
    setShowNotesPanel(false);
  }, []);

  const handleJumpToLine = useCallback((lineNumber: number) => {
    const lineEl = lineRefs.current.get(lineNumber);
    if (lineEl) {
      lineEl.scrollIntoView({ behavior: 'smooth', block: 'center' });
      setHighlightedLine(lineNumber);
    }
    setShowNotesPanel(false);
  }, []);

  // Format and send notes (REQ-PF-009 format)
  const handleSendFeedback = useCallback(() => {
    if (notes.length === 0) return;

    const formatted =
      `Review notes for \`task\`:\n\n` +
      notes
        .map((n) => `> Line ${n.lineNumber}: \`${n.lineContent}\`\n${n.note}`)
        .join('\n\n');

    onSendFeedback(formatted);
    setNotes([]);
    setShowNotesPanel(false);
  }, [notes, onSendFeedback]);

  const handleDiscard = useCallback(() => {
    setDiscardConfirmOpen(true);
  }, []);

  const handleApprove = useCallback(
    (handoff: TaskApprovalHandoff) => {
      setApprovingHandoff(handoff);
      onApprove(handoff);
    },
    [onApprove]
  );

  const confirmDiscard = useCallback(() => {
    setDiscardConfirmOpen(false);
    onReject();
  }, [onReject]);

  // Render plan as markdown with annotatable blocks
  const renderPlanMarkdown = useMemo(() => {
    const rawLines = plan.split('\n');

    const annotatable = (Tag: React.ElementType) =>
      ({
        children,
        node,
        ...props
      }: {
        children?: React.ReactNode;
        node?: {
          position?: {
            start?: { line?: number };
            end?: { line?: number };
          };
        };
        [key: string]: unknown;
      }) => {
        const ln = node?.position?.start?.line ?? 0;
        const startLine = (node?.position?.start?.line ?? 1) - 1;
        const endLine = (node?.position?.end?.line ?? startLine + 1) - 1;
        const rawLineContent = rawLines
          .slice(startLine, endLine + 1)
          .join(' ')
          .slice(0, 200);
        return (
          <AnnotatableBlock
            as={Tag}
            lineNumber={ln}
            lineContent={rawLineContent}
            onAnnotate={handleLongPress}
            className="viewer-markdown-block"
            isHighlighted={highlightedLine === ln}
            lineRef={(el) => {
              if (el) lineRefs.current.set(ln, el);
            }}
            {...props}
          >
            {children}
          </AnnotatableBlock>
        );
      };

    return (
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={
          {
            p: annotatable('p'),
            h1: annotatable('h1'),
            h2: annotatable('h2'),
            h3: annotatable('h3'),
            td: annotatable('td'),
            th: annotatable('th'),
            li: annotatable('li'),
            blockquote: annotatable('blockquote'),
            code: ({
              inline,
              className,
              children,
              ...props
            }: {
              inline?: boolean;
              className?: string;
              children?: React.ReactNode;
              [key: string]: unknown;
            }) => {
              const match = /language-([^\s]+)/.exec(className || '');
              const language = match?.[1]?.toLowerCase();
              if (!inline && language === 'mermaid') {
                return <MermaidDiagram code={String(children)} />;
              }
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
          } as unknown as Components
        }
      >
        {plan}
      </ReactMarkdown>
    );
  }, [plan, highlightedLine, handleLongPress]);

  return (
    <div className="task-approval-reader">
      {/* Header */}
      <div className="task-approval-header">
        <div className="task-approval-title-row">
          <h2 className="task-approval-title">{title}</h2>
          <span className="task-approval-priority">{priority}</span>
        </div>
        <div className="task-approval-header-actions">
          {notes.length > 0 && (
            <>
              <button
                className="task-approval-badge"
                onClick={() => setShowNotesPanel(!showNotesPanel)}
                aria-label={`${notes.length} notes`}
              >
                <MessageSquare size={18} />
                <span>{notes.length}</span>
              </button>
            </>
          )}
          <button
            className="task-approval-header-discard"
            onClick={handleDiscard}
            aria-label="Discard task"
            title="Discard task"
          >
            <X size={18} />
          </button>
        </div>
      </div>

      {/* Plan content */}
      <div className="task-approval-content">
        <div className="viewer-markdown">{renderPlanMarkdown}</div>
      </div>

      {hasUnsentNotes && (
        <div className="task-approval-feedback-cue" role="status">
          You have {noteCountLabel} of unsent feedback. Send feedback, or approve
          without sending those notes.
        </div>
      )}

      {contextUsagePercent && (
        <div className="task-approval-context-cue">
          <span className="task-approval-context-cue__label">Context used</span>
          <span className="task-approval-context-cue__value">{contextUsagePercent}</span>
          <span className="task-approval-context-cue__hint">
            Start here keeps this Explore context in play; New chat starts a continuation.
          </span>
        </div>
      )}

      {/* Action toolbar */}
      <div className="task-approval-actions">
        <button
          className={[
            'task-approval-btn',
            'task-approval-btn--feedback',
            hasUnsentNotes && 'task-approval-btn--recommended',
          ]
            .filter(Boolean)
            .join(' ')}
          onClick={handleSendFeedback}
          disabled={!hasUnsentNotes}
          aria-label={`Request changes (${notes.length})`}
          title={
            !hasUnsentNotes
              ? 'Add annotations to the plan before sending feedback'
              : `Send ${noteCountLabel} as feedback`
          }
        >
          <Send size={18} />
          <span className="task-approval-btn-label-full">Request changes ({notes.length})</span>
          <span className="task-approval-btn-label-compact">Revise</span>
        </button>
        <button
          className={[
            'task-approval-btn',
            'task-approval-btn--approve',
            hasUnsentNotes && 'task-approval-btn--subdued',
          ]
            .filter(Boolean)
            .join(' ')}
          disabled={approvingHandoff !== null}
          onClick={() => handleApprove('continue_in_current_conversation')}
          aria-label="Approve and start here"
        >
          {approvingHandoff === 'continue_in_current_conversation' ? (
            <>
              <Loader2 size={18} className="spinning" />
              Approving...
            </>
          ) : (
            <>
              <Check size={18} />
              <span className="task-approval-btn-label-full">Start here</span>
              <span className="task-approval-btn-label-compact">Start here</span>
            </>
          )}
        </button>
        <button
          className={[
            'task-approval-btn',
            'task-approval-btn--approve',
            hasUnsentNotes && 'task-approval-btn--subdued',
          ]
            .filter(Boolean)
            .join(' ')}
          disabled={approvingHandoff !== null}
          onClick={() => handleApprove('start_fresh_work_conversation')}
          aria-label="Approve and start a new continuation conversation"
        >
          {approvingHandoff === 'start_fresh_work_conversation' ? (
            <>
              <Loader2 size={18} className="spinning" />
              Approving...
            </>
          ) : (
            <>
              <Check size={18} />
              <span className="task-approval-btn-label-full">New chat</span>
              <span className="task-approval-btn-label-compact">New chat</span>
            </>
          )}
        </button>
      </div>

      {/* Annotation Dialog */}
      {annotatingLine && (
        <div
          className="task-approval-annotation-overlay"
          onClick={() => setAnnotatingLine(null)}
        >
          <div
            className="task-approval-annotation-dialog"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="task-approval-annotation-header">
              <span>Line {annotatingLine.lineNumber}</span>
              <button onClick={() => setAnnotatingLine(null)}>
                <X size={18} />
              </button>
            </div>
            <div className="task-approval-annotation-preview">
              {annotatingLine.lineContent.slice(0, 100)}
              {annotatingLine.lineContent.length > 100 && '...'}
            </div>
            <textarea
              ref={noteInputRef}
              className="task-approval-annotation-input"
              placeholder="Add your note..."
              value={noteInput}
              onChange={(e) => setNoteInput(e.target.value)}
              rows={3}
            />
            <div className="task-approval-annotation-actions">
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

      {/* Notes Panel */}
      {showNotesPanel && (
        <div className="task-approval-notes-panel">
          <div className="task-approval-notes-header">
            <span>Notes ({notes.length})</span>
            <button onClick={() => setShowNotesPanel(false)}>
              <ChevronDown size={18} />
            </button>
          </div>
          <div className="task-approval-notes-list">
            {notes.map((note) => (
              <div key={note.id} className="task-approval-note">
                <div className="task-approval-note-header">
                  <button
                    className="task-approval-note-line"
                    onClick={() => handleJumpToLine(note.lineNumber)}
                  >
                    Line {note.lineNumber}
                  </button>
                  <button
                    className="task-approval-note-delete"
                    onClick={() => handleDeleteNote(note.id)}
                  >
                    <Trash2 size={14} />
                  </button>
                </div>
                <div className="task-approval-note-preview">
                  {note.lineContent.slice(0, 60)}
                  {note.lineContent.length > 60 && '...'}
                </div>
                <div className="task-approval-note-text">{note.note}</div>
              </div>
            ))}
          </div>
          <div className="task-approval-notes-actions">
            <button onClick={handleClearAll}>Clear All</button>
            <button className="primary" onClick={handleSendFeedback}>
              <Send size={16} />
              Send All
            </button>
          </div>
        </div>
      )}

      {/* Discard Confirmation */}
      {discardConfirmOpen && (
        <div className="task-approval-confirm-overlay">
          <div
            className="task-approval-confirm-dialog"
            onClick={(e) => e.stopPropagation()}
          >
            <p>
              Discard this task? The agent will be informed the task was
              rejected.
            </p>
            <div className="task-approval-confirm-actions">
              <button onClick={() => setDiscardConfirmOpen(false)}>
                Cancel
              </button>
              <button className="danger" onClick={confirmDiscard}>
                Discard
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
