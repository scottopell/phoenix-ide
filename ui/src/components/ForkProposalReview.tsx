/**
 * ForkProposalReview
 *
 * Full-screen review surface for a decoupled task fork proposal
 * (REQ-PROJ-034 / 037). Reuses the Explore task-approval shell (the
 * `task-approval-*` classes + non-dismissible phase-overlay structure) so the
 * fork review reads identically to plan review, rather than introducing a new
 * design language. The snapshot — title, priority, task_file, body — comes from
 * the proposal record (the body is not in the transcript).
 *
 * Three actions: Approve (spawn the Work fork), Dismiss, and Request Changes
 * (reveals a required free-text note, then promotes to an Explore refinement).
 */

import { useState, useEffect, useCallback, useMemo } from 'react';
import ReactMarkdown from 'react-markdown';
import type { Components } from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { SyntaxHighlighter, oneDark } from '../utils/syntaxHighlighter';
import { useRegisterFocusScope } from '../hooks/useFocusScope';
import { Check, XCircle, MessageSquarePlus, Send, Loader2 } from 'lucide-react';
import type { ForkProposalSummary } from '../api';

export interface ForkProposalReviewProps {
  proposal: ForkProposalSummary;
  onApprove: () => void | Promise<void>;
  onDismiss: () => void | Promise<void>;
  onRequestChanges: (note: string) => void | Promise<void>;
  onClose: () => void;
}

type Busy = 'approve' | 'dismiss' | 'request-changes' | null;

export function ForkProposalReview({
  proposal,
  onApprove,
  onDismiss,
  onRequestChanges,
  onClose,
}: ForkProposalReviewProps) {
  useRegisterFocusScope('fork-proposal-review');

  const [busy, setBusy] = useState<Busy>(null);
  const [showNoteInput, setShowNoteInput] = useState(false);
  const [note, setNote] = useState('');

  // Escape closes the review (unlike Explore approval, a fork proposal is
  // non-blocking — leaving it un-reviewed is harmless, so dismissal-by-Escape
  // simply returns to the transcript without resolving the proposal).
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && busy === null) {
        e.preventDefault();
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener('keydown', handleKeyDown, true);
    return () => window.removeEventListener('keydown', handleKeyDown, true);
  }, [busy, onClose]);

  const handleApprove = useCallback(async () => {
    setBusy('approve');
    await onApprove();
  }, [onApprove]);

  const handleDismiss = useCallback(async () => {
    setBusy('dismiss');
    await onDismiss();
  }, [onDismiss]);

  const handleSubmitNote = useCallback(async () => {
    if (!note.trim()) return;
    setBusy('request-changes');
    await onRequestChanges(note.trim());
  }, [note, onRequestChanges]);

  const renderBody = useMemo(
    () => (
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={
          {
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
              const match = /language-(\w+)/.exec(className || '');
              return !inline && match ? (
                <SyntaxHighlighter style={oneDark} language={match[1]} PreTag="div" {...props}>
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
        {proposal.body}
      </ReactMarkdown>
    ),
    [proposal.body],
  );

  return (
    <div className="task-approval-reader">
      {/* Header */}
      <div className="task-approval-header">
        <div className="task-approval-title-row">
          <h2 className="task-approval-title">{proposal.title}</h2>
          <span className="task-approval-priority">{proposal.priority}</span>
        </div>
      </div>

      {/* File path — where the fork will commit the brief. */}
      <div className="task-approval-subhead">
        <code>{proposal.task_file}</code>
      </div>

      {/* Brief body */}
      <div className="task-approval-content">
        <div className="viewer-markdown">{renderBody}</div>
      </div>

      {/* Request-changes note input, revealed on demand. */}
      {showNoteInput && (
        <div className="fork-review-note">
          <label className="fork-review-note__label" htmlFor="fork-review-note-input">
            Describe the changes the refinement should make:
          </label>
          <textarea
            id="fork-review-note-input"
            className="fork-review-note__input"
            placeholder="e.g. narrow the scope to the parser only, add acceptance criteria…"
            value={note}
            onChange={(e) => setNote(e.target.value)}
            rows={3}
            autoFocus
          />
        </div>
      )}

      {/* Action toolbar */}
      <div className="task-approval-actions">
        <button
          className="task-approval-btn task-approval-btn--discard"
          onClick={handleDismiss}
          disabled={busy !== null}
        >
          {busy === 'dismiss' ? <Loader2 size={18} className="spinning" /> : <XCircle size={18} />}
          Dismiss
        </button>

        {showNoteInput ? (
          <button
            className="task-approval-btn task-approval-btn--feedback"
            onClick={handleSubmitNote}
            disabled={busy !== null || !note.trim()}
            title={!note.trim() ? 'Enter a change request before sending' : 'Send change request'}
          >
            {busy === 'request-changes' ? (
              <Loader2 size={18} className="spinning" />
            ) : (
              <Send size={18} />
            )}
            Send Request
          </button>
        ) : (
          <button
            className="task-approval-btn task-approval-btn--feedback"
            onClick={() => setShowNoteInput(true)}
            disabled={busy !== null}
          >
            <MessageSquarePlus size={18} />
            Request Changes
          </button>
        )}

        <button
          className="task-approval-btn task-approval-btn--approve"
          onClick={handleApprove}
          disabled={busy !== null}
        >
          {busy === 'approve' ? (
            <>
              <Loader2 size={18} className="spinning" />
              Approving…
            </>
          ) : (
            <>
              <Check size={18} />
              Approve
            </>
          )}
        </button>
      </div>
    </div>
  );
}
