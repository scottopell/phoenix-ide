/**
 * QuestionPanel Component
 *
 * Renders when the conversation is in `awaiting_user_response` state.
 * Displays agent-posed questions with radio/checkbox options, optional
 * previews, an "Other" free-text option, and per-question notes.
 *
 * Submit calls api.respondToQuestion; Decline calls api.cancelConversation.
 */

import { useState, useCallback, useEffect } from 'react';
import { api } from '../api';
import type { UserQuestion } from '../api';
import { Check, XCircle, ChevronDown, ChevronRight } from 'lucide-react';
import { ConfirmDialog } from './ConfirmDialog';
import './QuestionPanel.css';

export interface QuestionPanelProps {
  questions: UserQuestion[];
  conversationId: string;
  showToast: (message: string, duration?: number) => void;
}

const OTHER_SENTINEL = '__other__';

function hasPreviewOptions(q: UserQuestion): boolean {
  return !q.multiSelect && q.options.some((o) => o.preview);
}

export function QuestionPanel({
  questions,
  conversationId,
  showToast,
}: QuestionPanelProps) {
  // Per-question selected value (label or OTHER_SENTINEL for single-select,
  // comma-joined labels for multi-select)
  const [answers, setAnswers] = useState<Record<string, string>>({});
  const [otherTexts, setOtherTexts] = useState<Record<string, string>>({});
  const [annotations, setAnnotations] = useState<
    Record<string, { notes?: string; preview?: string }>
  >({});
  const [submitting, setSubmitting] = useState(false);
  const [focusedPreviews, setFocusedPreviews] = useState<Record<string, string>>({});
  const [expandedNotes, setExpandedNotes] = useState<Record<string, boolean>>({});
  const [feedback, setFeedback] = useState<{
    message: string;
    isError: boolean;
  } | null>(null);
  const [showConfirmDecline, setShowConfirmDecline] = useState(false);

  // Multi-select: track selected labels as a Set per question
  const [multiSelections, setMultiSelections] = useState<Record<string, Set<string>>>({});

  const setAnswer = useCallback((questionText: string, value: string) => {
    setAnswers((prev) => ({ ...prev, [questionText]: value }));
    setFeedback(null);
  }, []);

  const setOtherText = useCallback((questionText: string, value: string) => {
    setOtherTexts((prev) => ({ ...prev, [questionText]: value }));
  }, []);

  const toggleMultiSelect = useCallback(
    (questionText: string, label: string) => {
      setMultiSelections((prev) => {
        const current = new Set(prev[questionText] ?? []);
        if (current.has(label)) {
          current.delete(label);
        } else {
          current.add(label);
        }
        return { ...prev, [questionText]: current };
      });
      setFeedback(null);
    },
    []
  );

  const toggleNotes = useCallback((questionText: string) => {
    setExpandedNotes((prev) => ({
      ...prev,
      [questionText]: !prev[questionText],
    }));
  }, []);

  const setNotes = useCallback((questionText: string, notes: string) => {
    setAnnotations((prev) => ({
      ...prev,
      [questionText]: { ...prev[questionText], notes: notes || undefined },
    }));
  }, []);

  // Escape key opens the decline confirmation dialog
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !showConfirmDecline) {
        setShowConfirmDecline(true);
      }
    };
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [showConfirmDecline]);

  // Check if every question has an answer
  const allAnswered = questions.every((q) => {
    if (q.multiSelect) {
      const sel = multiSelections[q.question];
      // Multi-select requires at least one selection (either a checkbox or "other" with text)
      if (!sel || sel.size === 0) {
        // Check if "other" is selected with text
        return (
          answers[q.question] === OTHER_SENTINEL &&
          (otherTexts[q.question] ?? '').trim().length > 0
        );
      }
      // If "other" is also selected, it needs text
      if (sel.has(OTHER_SENTINEL)) {
        return (otherTexts[q.question] ?? '').trim().length > 0;
      }
      return true;
    }
    const answer = answers[q.question];
    if (!answer) return false;
    if (answer === OTHER_SENTINEL) {
      return (otherTexts[q.question] ?? '').trim().length > 0;
    }
    return true;
  });

  const buildAnswerMap = useCallback((): Record<string, string> => {
    const result: Record<string, string> = {};
    for (const q of questions) {
      if (q.multiSelect) {
        const sel = multiSelections[q.question] ?? new Set();
        const labels = Array.from(sel).filter((l) => l !== OTHER_SENTINEL);
        if (sel.has(OTHER_SENTINEL) && (otherTexts[q.question] ?? '').trim()) {
          labels.push(otherTexts[q.question]!.trim());
        }
        result[q.question] = labels.join(', ');
      } else {
        const answer = answers[q.question];
        if (answer === OTHER_SENTINEL) {
          result[q.question] = (otherTexts[q.question] ?? '').trim();
        } else {
          result[q.question] = answer ?? '';
        }
      }
    }
    return result;
  }, [questions, answers, otherTexts, multiSelections]);

  const buildAnnotations = useCallback(():
    | Record<string, { notes?: string; preview?: string }>
    | undefined => {
    // Include preview content for single-select questions with previews
    const result: Record<string, { notes?: string; preview?: string }> = {};
    let hasAny = false;

    for (const q of questions) {
      const notes = annotations[q.question]?.notes;
      let preview: string | undefined;

      if (!q.multiSelect) {
        const selectedLabel = answers[q.question];
        if (selectedLabel && selectedLabel !== OTHER_SENTINEL) {
          const opt = q.options.find((o) => o.label === selectedLabel);
          if (opt?.preview) {
            preview = opt.preview;
          }
        }
      }

      if (notes || preview) {
        result[q.question] = { notes, preview };
        hasAny = true;
      }
    }

    return hasAny ? result : undefined;
  }, [questions, answers, annotations]);

  const handleSubmit = useCallback(async () => {
    if (!allAnswered || submitting) return;
    setSubmitting(true);
    setFeedback(null);
    try {
      await api.respondToQuestion(
        conversationId,
        buildAnswerMap(),
        buildAnnotations()
      );
      showToast('Response sent', 3000);
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Failed to submit response';
      setFeedback({ message: msg, isError: true });
    } finally {
      setSubmitting(false);
    }
  }, [
    allAnswered,
    submitting,
    conversationId,
    buildAnswerMap,
    buildAnnotations,
    showToast,
  ]);

  const handleDeclineClick = useCallback(() => {
    if (submitting) return;
    setShowConfirmDecline(true);
  }, [submitting]);

  const handleConfirmDecline = useCallback(async () => {
    setShowConfirmDecline(false);
    if (submitting) return;
    setSubmitting(true);
    setFeedback(null);
    try {
      await api.cancelConversation(conversationId);
      showToast('Declined to answer', 3000);
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Failed to decline';
      setFeedback({ message: msg, isError: true });
    } finally {
      setSubmitting(false);
    }
  }, [submitting, conversationId, showToast]);

  return (
    <div className="question-panel">
      <span className="question-panel-label">Agent needs your input</span>

      <div className="question-panel-content">
        {questions.map((q) => (
          <QuestionItem
            key={q.question}
            question={q}
            answer={answers[q.question]}
            otherText={otherTexts[q.question] ?? ''}
            multiSelected={multiSelections[q.question] ?? new Set()}
            focusedPreview={focusedPreviews[q.question]}
            notesExpanded={expandedNotes[q.question] ?? false}
            notesText={annotations[q.question]?.notes ?? ''}
            onSelect={setAnswer}
            onOtherText={setOtherText}
            onMultiToggle={toggleMultiSelect}
            onFocusPreview={(questionText, preview) =>
              setFocusedPreviews((prev) => ({ ...prev, [questionText]: preview }))
            }
            onToggleNotes={toggleNotes}
            onSetNotes={setNotes}
          />
        ))}
      </div>

      <div className="question-actions">
        <button
          className="question-btn question-btn--decline"
          onClick={handleDeclineClick}
          disabled={submitting}
        >
          <XCircle size={18} />
          Decline
        </button>
        <ConfirmDialog
          visible={showConfirmDecline}
          title="Decline to answer?"
          message="The agent will proceed using its own judgment."
          confirmText="Decline"
          cancelText="Cancel"
          danger
          onConfirm={handleConfirmDecline}
          onCancel={() => setShowConfirmDecline(false)}
        />
        <div className="question-actions-right">
          {feedback && (
            <span
              className={`question-feedback${feedback.isError ? ' question-feedback--error' : ''}`}
            >
              {feedback.message}
            </span>
          )}
          <button
            className="question-btn question-btn--submit"
            onClick={handleSubmit}
            disabled={!allAnswered || submitting}
            title={
              !allAnswered
                ? 'Answer all questions before submitting'
                : undefined
            }
          >
            <Check size={18} />
            {submitting ? 'Sending...' : 'Submit'}
          </button>
        </div>
      </div>
    </div>
  );
}

// --- Individual Question ---

interface QuestionItemProps {
  question: UserQuestion;
  answer: string | undefined;
  otherText: string;
  multiSelected: Set<string>;
  focusedPreview: string | undefined;
  notesExpanded: boolean;
  notesText: string;
  onSelect: (questionText: string, value: string) => void;
  onOtherText: (questionText: string, value: string) => void;
  onMultiToggle: (questionText: string, label: string) => void;
  onFocusPreview: (questionText: string, preview: string) => void;
  onToggleNotes: (questionText: string) => void;
  onSetNotes: (questionText: string, notes: string) => void;
}

function QuestionItem({
  question: q,
  answer,
  otherText,
  multiSelected,
  focusedPreview,
  notesExpanded,
  notesText,
  onSelect,
  onOtherText,
  onMultiToggle,
  onFocusPreview,
  onToggleNotes,
  onSetNotes,
}: QuestionItemProps) {
  const isPreviewMode = hasPreviewOptions(q);
  const isMulti = q.multiSelect;

  // Determine which preview to show
  const activePreview = (() => {
    if (!isPreviewMode) return undefined;
    if (focusedPreview) return focusedPreview;
    // Fall back to selected option's preview
    if (answer && answer !== OTHER_SENTINEL) {
      const opt = q.options.find((o) => o.label === answer);
      return opt?.preview;
    }
    // Default: first option with a preview
    return q.options.find((o) => o.preview)?.preview;
  })();

  const renderOptions = () => {
    const optionElements = q.options.map((opt) => {
      if (isMulti) {
        const checked = multiSelected.has(opt.label);
        return (
          <label
            key={opt.label}
            className={`question-option${checked ? ' selected' : ''}`}
          >
            <input
              type="checkbox"
              checked={checked}
              onChange={() => onMultiToggle(q.question, opt.label)}
            />
            <div className="question-option-content">
              <span className="question-option-label">{opt.label}</span>
              {opt.description && (
                <span className="question-option-description">
                  {opt.description}
                </span>
              )}
            </div>
          </label>
        );
      }

      const selected = answer === opt.label;
      return (
        <label
          key={opt.label}
          className={`question-option${selected ? ' selected' : ''}`}
          onMouseEnter={() => {
            if (isPreviewMode && opt.preview) {
              onFocusPreview(q.question, opt.preview);
            }
          }}
        >
          <input
            type="radio"
            name={q.question}
            checked={selected}
            onChange={() => onSelect(q.question, opt.label)}
          />
          <div className="question-option-content">
            <span className="question-option-label">{opt.label}</span>
            {opt.description && (
              <span className="question-option-description">
                {opt.description}
              </span>
            )}
          </div>
        </label>
      );
    });

    // "Other" option
    const otherSelected = isMulti
      ? multiSelected.has(OTHER_SENTINEL)
      : answer === OTHER_SENTINEL;

    const otherElement = (
      <div
        key="__other__"
        className={`question-other${otherSelected ? ' selected' : ''}`}
      >
        <input
          type={isMulti ? 'checkbox' : 'radio'}
          name={q.question}
          checked={otherSelected}
          onChange={() => {
            if (isMulti) {
              onMultiToggle(q.question, OTHER_SENTINEL);
            } else {
              onSelect(q.question, OTHER_SENTINEL);
            }
          }}
        />
        <input
          type="text"
          className="question-other-input"
          placeholder="Other..."
          value={otherText}
          onChange={(e) => onOtherText(q.question, e.target.value)}
          onFocus={() => {
            if (isMulti) {
              if (!multiSelected.has(OTHER_SENTINEL)) {
                onMultiToggle(q.question, OTHER_SENTINEL);
              }
            } else {
              onSelect(q.question, OTHER_SENTINEL);
            }
          }}
        />
      </div>
    );

    return [...optionElements, otherElement];
  };

  return (
    <div className="question-item">
      <span className="question-header">{q.header}</span>
      <span className="question-text">{q.question}</span>

      {isPreviewMode ? (
        <div className="question-preview-layout">
          <div className="question-options">{renderOptions()}</div>
          <div
            className={`question-preview-pane${!activePreview ? ' question-preview-pane--empty' : ''}`}
          >
            {activePreview || 'Select an option to preview'}
          </div>
        </div>
      ) : (
        <div className="question-options">{renderOptions()}</div>
      )}

      {/* Notes only for single-select with previews -- users comparing
          concrete artifacts may want to qualify their selection. For plain
          single-select or multi-select, "Other" free-text is sufficient. */}
      {isPreviewMode && (
        <div className="question-notes">
          <button
            className="question-notes-toggle"
            onClick={() => onToggleNotes(q.question)}
          >
            {notesExpanded ? (
              <ChevronDown size={14} />
            ) : (
              <ChevronRight size={14} />
            )}
            Add notes
          </button>
          {notesExpanded && (
            <textarea
              placeholder="Optional notes for the agent..."
              value={notesText}
              onChange={(e) => onSetNotes(q.question, e.target.value)}
              rows={2}
            />
          )}
        </div>
      )}
    </div>
  );
}
