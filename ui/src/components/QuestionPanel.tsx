/**
 * QuestionPanel Component
 *
 * Renders when the conversation is in `awaiting_user_response` state.
 * Step-by-step wizard showing one question at a time with full keyboard
 * navigation (arrow keys, space, enter, tab, escape).
 *
 * Submit calls api.respondToQuestion; Decline calls api.cancelConversation.
 */

import { useState, useCallback, useEffect, useRef } from 'react';
import { api } from '../api';
import type { UserQuestion } from '../api';
import {
  Check,
  ChevronDown,
  ChevronRight,
} from 'lucide-react';
import { ConfirmDialog } from './ConfirmDialog';
import { useRegisterFocusScope } from '../hooks/useFocusScope';
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

/** Total number of focusable options for a question (predefined + Other) */
function optionCount(q: UserQuestion): number {
  return q.options.length + 1; // +1 for "Other"
}

export function QuestionPanel({
  questions,
  conversationId,
  showToast,
}: QuestionPanelProps) {
  useRegisterFocusScope('question-panel');

  // --- Wizard step state ---
  const [currentStep, setCurrentStep] = useState(0);
  const [focusedIndex, setFocusedIndex] = useState(0);
  const [enterPressedOnLast, setEnterPressedOnLast] = useState(false);

  // --- Existing answer state ---
  const [answers, setAnswers] = useState<Record<string, string>>(() => {
    const initial: Record<string, string> = {};
    for (const q of questions) {
      if (!q.multiSelect && hasPreviewOptions(q) && q.options.length > 0) {
        initial[q.question] = q.options[0].label;
      }
    }
    return initial;
  });
  const [otherTexts, setOtherTexts] = useState<Record<string, string>>({});
  const [annotations, setAnnotations] = useState<
    Record<string, { notes?: string; preview?: string }>
  >({});
  const [submitting, setSubmitting] = useState(false);
  const [focusedPreviews, setFocusedPreviews] = useState<
    Record<string, string>
  >({});
  const [expandedNotes, setExpandedNotes] = useState<Record<string, boolean>>(
    {}
  );
  const [feedback, setFeedback] = useState<{
    message: string;
    isError: boolean;
  } | null>(null);
  const [showConfirmDecline, setShowConfirmDecline] = useState(false);
  const [multiSelections, setMultiSelections] = useState<
    Record<string, Set<string>>
  >({});

  const otherInputRef = useRef<HTMLInputElement>(null);

  const currentQuestion = questions[currentStep];
  const isLastStep = currentStep === questions.length - 1;
  const isFirstStep = currentStep === 0;
  const totalSteps = questions.length;

  // --- Answer callbacks ---
  const setAnswer = useCallback(
    (questionText: string, value: string) => {
      setAnswers((prev) => ({ ...prev, [questionText]: value }));
      setFeedback(null);
      setEnterPressedOnLast(false);
    },
    []
  );

  const setOtherText = useCallback((questionText: string, value: string) => {
    setOtherTexts((prev) => ({ ...prev, [questionText]: value }));
    setEnterPressedOnLast(false);
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
      setEnterPressedOnLast(false);
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

  // --- allAnswered check ---
  const allAnswered = questions.every((q) => {
    if (q.multiSelect) {
      const sel = multiSelections[q.question];
      if (!sel || sel.size === 0) {
        return (
          answers[q.question] === OTHER_SENTINEL &&
          (otherTexts[q.question] ?? '').trim().length > 0
        );
      }
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

  // --- Build answer map & annotations ---
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

  // --- Submit / Decline ---
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
      const msg =
        err instanceof Error ? err.message : 'Failed to submit response';
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

  // --- Navigation ---
  const goToStep = useCallback(
    (step: number) => {
      if (step < 0 || step >= totalSteps) return;
      const targetQ = questions[step];
      // Determine initial focus: previously selected option, or first option
      let initialFocus = 0;
      const ans = answers[targetQ.question];
      if (ans) {
        if (ans === OTHER_SENTINEL) {
          initialFocus = targetQ.options.length; // "Other" is last
        } else {
          const idx = targetQ.options.findIndex((o) => o.label === ans);
          if (idx >= 0) initialFocus = idx;
        }
      } else if (targetQ.multiSelect) {
        const sel = multiSelections[targetQ.question];
        if (sel && sel.size > 0) {
          // Focus the first selected option
          const firstSelected = targetQ.options.findIndex((o) =>
            sel.has(o.label)
          );
          if (firstSelected >= 0) initialFocus = firstSelected;
        }
      }
      setCurrentStep(step);
      setFocusedIndex(initialFocus);
      setEnterPressedOnLast(false);
    },
    [totalSteps, questions, answers, multiSelections]
  );

  const goNext = useCallback(() => {
    if (!isLastStep) {
      goToStep(currentStep + 1);
    }
  }, [isLastStep, currentStep, goToStep]);

  const goBack = useCallback(() => {
    if (!isFirstStep) {
      goToStep(currentStep - 1);
    }
  }, [isFirstStep, currentStep, goToStep]);

  // --- Focus management: auto-focus on mount and step change ---
  useEffect(() => {
    if (!currentQuestion) return;
    // Determine initial focus for this step
    let initialFocus = 0;
    const ans = answers[currentQuestion.question];
    if (ans) {
      if (ans === OTHER_SENTINEL) {
        initialFocus = currentQuestion.options.length;
      } else {
        const idx = currentQuestion.options.findIndex(
          (o) => o.label === ans
        );
        if (idx >= 0) initialFocus = idx;
      }
    }
    setFocusedIndex(initialFocus);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []); // Only on mount

  // --- Update focused preview when focusedIndex changes ---
  useEffect(() => {
    if (!currentQuestion) return;
    const isPreviewMode = hasPreviewOptions(currentQuestion);
    if (!isPreviewMode) return;

    if (focusedIndex < currentQuestion.options.length) {
      const opt = currentQuestion.options[focusedIndex];
      if (opt?.preview) {
        setFocusedPreviews((prev) => ({
          ...prev,
          [currentQuestion.question]: opt.preview!,
        }));
      }
    }
  }, [focusedIndex, currentStep, currentQuestion]);

  // --- Select/toggle the focused option ---
  const selectFocusedOption = useCallback(() => {
    if (!currentQuestion) return;
    const isOther = focusedIndex >= currentQuestion.options.length;

    if (isOther) {
      if (currentQuestion.multiSelect) {
        toggleMultiSelect(currentQuestion.question, OTHER_SENTINEL);
        // Focus the text input
        setTimeout(() => otherInputRef.current?.focus(), 0);
      } else {
        setAnswer(currentQuestion.question, OTHER_SENTINEL);
        setTimeout(() => otherInputRef.current?.focus(), 0);
      }
    } else {
      const opt = currentQuestion.options[focusedIndex];
      if (!opt) return;
      if (currentQuestion.multiSelect) {
        toggleMultiSelect(currentQuestion.question, opt.label);
      } else {
        setAnswer(currentQuestion.question, opt.label);
      }
    }
  }, [currentQuestion, focusedIndex, toggleMultiSelect, setAnswer]);

  // --- Keyboard handler ---
  useEffect(() => {
    if (!currentQuestion) return;

    const handler = (e: KeyboardEvent) => {
      // If confirm dialog is open, don't handle
      if (showConfirmDecline) return;

      const isInInput =
        e.target instanceof HTMLInputElement ||
        e.target instanceof HTMLTextAreaElement;

      // Always handle Ctrl/Cmd+Enter for submit
      if (
        e.key === 'Enter' &&
        (e.ctrlKey || e.metaKey)
      ) {
        e.preventDefault();
        handleSubmit();
        return;
      }

      // If typing in a text input, don't capture other keys
      if (isInInput) return;

      const count = optionCount(currentQuestion);

      switch (e.key) {
        case 'ArrowDown': {
          e.preventDefault();
          setFocusedIndex((prev) => Math.min(prev + 1, count - 1));
          break;
        }
        case 'ArrowUp': {
          e.preventDefault();
          setFocusedIndex((prev) => Math.max(prev - 1, 0));
          break;
        }
        case ' ': {
          e.preventDefault();
          selectFocusedOption();
          break;
        }
        case 'Enter': {
          e.preventDefault();
          if (isLastStep) {
            if (enterPressedOnLast) {
              // Second press: submit
              handleSubmit();
            } else {
              // First press: show toast
              setEnterPressedOnLast(true);
              showToast('Press Enter again to submit, or Ctrl+Enter', 3000);
            }
          } else {
            goNext();
          }
          break;
        }
        case 'Tab': {
          e.preventDefault();
          if (e.shiftKey) {
            goBack();
          } else {
            if (!isLastStep) {
              goNext();
            }
          }
          break;
        }
        case 'Escape': {
          e.preventDefault();
          setShowConfirmDecline(true);
          break;
        }
      }
    };

    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, [
    currentQuestion,
    currentStep,
    focusedIndex,
    isLastStep,
    enterPressedOnLast,
    showConfirmDecline,
    handleSubmit,
    selectFocusedOption,
    goNext,
    goBack,
    showToast,
  ]);

  // Per-question answered check for breadcrumb indicators
  const isQuestionAnswered = useCallback(
    (q: UserQuestion): boolean => {
      if (q.multiSelect) {
        const sel = multiSelections[q.question];
        if (!sel || sel.size === 0) {
          return (
            answers[q.question] === OTHER_SENTINEL &&
            (otherTexts[q.question] ?? '').trim().length > 0
          );
        }
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
    },
    [answers, otherTexts, multiSelections]
  );

  if (!currentQuestion) return null;

  return (
    <div className="question-panel">
      {/* Breadcrumb navigation: each question's header as a clickable tab */}
      {totalSteps > 1 && (
        <div className="question-breadcrumbs">
          {questions.map((q, i) => {
            const isCurrent = i === currentStep;
            const answered = isQuestionAnswered(q);
            return (
              <span key={q.question} className="question-breadcrumb-item">
                {i > 0 && (
                  <ChevronRight
                    size={12}
                    className="question-breadcrumb-separator"
                  />
                )}
                <button
                  className={`question-breadcrumb${isCurrent ? ' current' : ''}${answered && !isCurrent ? ' answered' : ''}${!answered && !isCurrent ? ' unanswered' : ''}`}
                  onClick={() => goToStep(i)}
                  disabled={submitting}
                  title={q.question}
                >
                  {answered && !isCurrent && (
                    <Check size={12} className="question-breadcrumb-check" />
                  )}
                  {q.header}
                </button>
              </span>
            );
          })}
        </div>
      )}

      <div className="question-wizard-content">
        <QuestionItem
          key={currentQuestion.question}
          question={currentQuestion}
          answer={answers[currentQuestion.question]}
          otherText={otherTexts[currentQuestion.question] ?? ''}
          multiSelected={multiSelections[currentQuestion.question] ?? new Set()}
          focusedPreview={focusedPreviews[currentQuestion.question]}
          notesExpanded={expandedNotes[currentQuestion.question] ?? false}
          notesText={annotations[currentQuestion.question]?.notes ?? ''}
          focusedIndex={focusedIndex}
          otherInputRef={otherInputRef}
          onSelect={setAnswer}
          onOtherText={setOtherText}
          onMultiToggle={toggleMultiSelect}
          onFocusPreview={(questionText, preview) =>
            setFocusedPreviews((prev) => ({
              ...prev,
              [questionText]: preview,
            }))
          }
          onToggleNotes={toggleNotes}
          onSetNotes={setNotes}
          onFocusIndex={setFocusedIndex}
        />
      </div>

      <div className="question-actions">
        <button
          className="question-btn question-btn--decline-small"
          onClick={handleDeclineClick}
          disabled={submitting}
        >
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
            <Check size={16} />
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
  focusedIndex: number;
  otherInputRef: React.RefObject<HTMLInputElement | null>;
  onSelect: (questionText: string, value: string) => void;
  onOtherText: (questionText: string, value: string) => void;
  onMultiToggle: (questionText: string, label: string) => void;
  onFocusPreview: (questionText: string, preview: string) => void;
  onToggleNotes: (questionText: string) => void;
  onSetNotes: (questionText: string, notes: string) => void;
  onFocusIndex: (index: number) => void;
}

function QuestionItem({
  question: q,
  answer,
  otherText,
  multiSelected,
  focusedPreview,
  notesExpanded,
  notesText,
  focusedIndex,
  otherInputRef,
  onSelect,
  onOtherText,
  onMultiToggle,
  onFocusPreview,
  onToggleNotes,
  onSetNotes,
  onFocusIndex,
}: QuestionItemProps) {
  const isPreviewMode = hasPreviewOptions(q);
  const isMulti = q.multiSelect;

  // Determine which preview to show: focused option's preview takes priority
  const activePreview = (() => {
    if (!isPreviewMode) return undefined;
    // Show preview for the focused option (keyboard navigation)
    if (focusedIndex < q.options.length) {
      const focusedOpt = q.options[focusedIndex];
      if (focusedOpt?.preview) return focusedOpt.preview;
    }
    // Fall back to hover-focused preview
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
    const optionElements = q.options.map((opt, index) => {
      const isFocused = focusedIndex === index;

      if (isMulti) {
        const checked = multiSelected.has(opt.label);
        return (
          <div
            key={opt.label}
            className={`question-option${checked ? ' selected' : ''}${isFocused ? ' focused' : ''}`}
            onClick={() => onMultiToggle(q.question, opt.label)}
            onMouseEnter={() => onFocusIndex(index)}
          >
            <input
              type="checkbox"
              checked={checked}
              onChange={() => onMultiToggle(q.question, opt.label)}
              tabIndex={-1}
            />
            <div className="question-option-content">
              <span className="question-option-label">{opt.label}</span>
              {opt.description && (
                <span className="question-option-description">
                  {opt.description}
                </span>
              )}
            </div>
          </div>
        );
      }

      const selected = answer === opt.label;
      return (
        <div
          key={opt.label}
          className={`question-option${selected ? ' selected' : ''}${isFocused ? ' focused' : ''}`}
          onClick={() => onSelect(q.question, opt.label)}
          onMouseEnter={() => {
            onFocusIndex(index);
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
            tabIndex={-1}
          />
          <div className="question-option-content">
            <span className="question-option-label">{opt.label}</span>
            {opt.description && (
              <span className="question-option-description">
                {opt.description}
              </span>
            )}
          </div>
        </div>
      );
    });

    // "Other" option
    const otherIndex = q.options.length;
    const isOtherFocused = focusedIndex === otherIndex;
    const otherSelected = isMulti
      ? multiSelected.has(OTHER_SENTINEL)
      : answer === OTHER_SENTINEL;

    const otherElement = (
      <div
        key="__other__"
        className={`question-other${otherSelected ? ' selected' : ''}${isOtherFocused ? ' focused' : ''}`}
        onClick={() => {
          if (isMulti) {
            onMultiToggle(q.question, OTHER_SENTINEL);
          } else {
            onSelect(q.question, OTHER_SENTINEL);
          }
          setTimeout(() => otherInputRef.current?.focus(), 0);
        }}
        onMouseEnter={() => onFocusIndex(otherIndex)}
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
          tabIndex={-1}
        />
        <input
          ref={otherInputRef}
          type="text"
          className="question-other-input"
          placeholder="Other..."
          value={otherText}
          onChange={(e) => onOtherText(q.question, e.target.value)}
          onFocus={() => {
            onFocusIndex(otherIndex);
            if (isMulti) {
              if (!multiSelected.has(OTHER_SENTINEL)) {
                onMultiToggle(q.question, OTHER_SENTINEL);
              }
            } else {
              onSelect(q.question, OTHER_SENTINEL);
            }
          }}
          tabIndex={-1}
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

      {/* Notes only for single-select with previews */}
      {isPreviewMode && (
        <div className="question-notes">
          <button
            className="question-notes-toggle"
            onClick={() => onToggleNotes(q.question)}
            tabIndex={-1}
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
