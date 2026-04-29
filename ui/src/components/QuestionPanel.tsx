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
  ArrowLeft,
  ArrowRight,
  Check,
  ChevronDown,
  ChevronRight,
} from 'lucide-react';
import ReactMarkdown from 'react-markdown';
import { ConfirmDialog } from './ConfirmDialog';
import { useRegisterFocusScope } from '../hooks/useFocusScope';
import { formatShortcut } from '../utils';
import './QuestionPanel.css';

export interface QuestionPanelProps {
  questions: UserQuestion[];
  conversationId: string;
  showToast: (message: string, duration?: number) => void;
  /** Called after a successful respond/cancel POST. The parent uses this to
   *  optimistically advance the local phase out of awaiting_user_response so
   *  the wizard dismisses immediately, instead of waiting for the SSE state
   *  echo (which can lag or be missed entirely on a flaky connection). The
   *  authoritative server-side phase change arrives via sse_state_change and
   *  reconciles. Mirrors handleSend in ConversationPage.tsx. */
  onSubmitted: () => void;
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
  onSubmitted,
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
        initial[q.question] = q.options[0]!.label;
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

  const otherInputRef = useRef<HTMLTextAreaElement>(null);

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
      [questionText]: {
        ...prev[questionText],
        notes: notes || undefined,
      },
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
        result[q.question] = {
          ...(notes ? { notes } : {}),
          ...(preview ? { preview } : {}),
        };
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
      onSubmitted();
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
    onSubmitted,
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
      onSubmitted();
      showToast('Declined to answer', 3000);
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Failed to decline';
      setFeedback({ message: msg, isError: true });
    } finally {
      setSubmitting(false);
    }
  }, [submitting, conversationId, onSubmitted, showToast]);

  // --- Navigation ---
  const goToStep = useCallback(
    (step: number) => {
      if (step < 0 || step >= totalSteps) return;
      const targetQ = questions[step];
      if (!targetQ) return;
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

  const panelRef = useRef<HTMLDivElement>(null);

  // --- Focus management: auto-focus on mount and step change (REQ-KB-004) ---
  useEffect(() => {
    if (!currentQuestion) return;
    // Focus the panel root so it receives keyboard events
    requestAnimationFrame(() => {
      panelRef.current?.focus();
    });
  }, [currentStep, currentQuestion]);

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

  // Per-question answered check (used by keyboard handler + breadcrumbs)
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

  // --- Keyboard handler (component-level, not document) ---
  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLDivElement>) => {
      if (!currentQuestion) return;

      // If confirm dialog is open, don't handle
      if (showConfirmDecline) return;

      const isInInput =
        e.target instanceof HTMLInputElement ||
        e.target instanceof HTMLTextAreaElement;

      // Always handle Ctrl/Cmd+Enter for submit, even in text inputs
      if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
        e.preventDefault();
        e.stopPropagation();
        handleSubmit();
        return;
      }

      // Always handle Escape, with context-dependent behavior
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        if (isInInput) {
          // Blur the notes textarea or other input
          (e.target as HTMLElement).blur();
          // Re-focus the panel so keyboard nav continues
          panelRef.current?.focus();
        } else {
          setShowConfirmDecline(true);
        }
        return;
      }

      // If typing in a text input (Other field), handle Enter for single-select.
      // Scope to HTMLInputElement only — textareas (notes, preview-Other) need
      // Enter to insert newlines, not navigate.
      if (isInInput) {
        if (e.target instanceof HTMLInputElement && e.key === 'Enter' && !e.shiftKey && !currentQuestion.multiSelect) {
          e.preventDefault();
          e.stopPropagation();
          if (!isLastStep) {
            setTimeout(() => goNext(), 200);
          } else {
            // Same double-enter-to-submit logic as regular Enter on last step
            const focusedIsOther =
              focusedIndex >= currentQuestion.options.length;
            const thisWillBeAnswered = focusedIsOther
              ? (otherTexts[currentQuestion.question] ?? '').trim().length > 0
              : true;
            const othersAnswered = questions.every((q, i) =>
              i === currentStep ? true : isQuestionAnswered(q)
            );
            const willAllBeAnswered = thisWillBeAnswered && othersAnswered;

            if (willAllBeAnswered) {
              if (enterPressedOnLast) {
                handleSubmit();
              } else {
                setEnterPressedOnLast(true);
                showToast(
                  'Press Enter again to submit, or Ctrl+Enter',
                  3000
                );
              }
            }
          }
        }
        return;
      }

      const count = optionCount(currentQuestion);
      const isMulti = currentQuestion.multiSelect;

      switch (e.key) {
        case 'ArrowDown': {
          e.preventDefault();
          e.stopPropagation();
          setFocusedIndex((prev) => Math.min(prev + 1, count - 1));
          break;
        }
        case 'ArrowUp': {
          e.preventDefault();
          e.stopPropagation();
          setFocusedIndex((prev) => Math.max(prev - 1, 0));
          break;
        }
        case ' ': {
          // Space: select/toggle focused option. Never auto-advances.
          e.preventDefault();
          e.stopPropagation();
          selectFocusedOption();
          break;
        }
        case 'Enter': {
          e.preventDefault();
          e.stopPropagation();

          if (isMulti) {
            // Multi-select: Enter toggles, no auto-advance
            selectFocusedOption();
          } else {
            // Single-select: Enter selects, then conditionally advances
            selectFocusedOption();

            if (isLastStep) {
              // Compute whether all questions will be answered after
              // this selection (current state doesn't reflect the
              // selection yet since setState is async).
              const focusedIsOther =
                focusedIndex >= currentQuestion.options.length;
              const thisWillBeAnswered = focusedIsOther
                ? (otherTexts[currentQuestion.question] ?? '').trim().length > 0
                : true;
              const othersAnswered = questions.every((q, i) =>
                i === currentStep ? true : isQuestionAnswered(q)
              );
              const willAllBeAnswered = thisWillBeAnswered && othersAnswered;

              if (willAllBeAnswered) {
                if (enterPressedOnLast) {
                  // Second press on last step: submit
                  handleSubmit();
                } else {
                  // First press on last step with all answered: toast
                  setEnterPressedOnLast(true);
                  showToast(
                    'Press Enter again to submit, or Ctrl+Enter',
                    3000
                  );
                }
              }
              // Last step but not all answered: just select, no submit attempt
            } else {
              // Not last step: auto-advance after 200ms delay
              setTimeout(() => goNext(), 200);
            }
          }
          break;
        }
        case 'Tab': {
          e.preventDefault();
          e.stopPropagation();
          if (e.shiftKey) {
            goBack();
          } else {
            goNext();
          }
          break;
        }
        case 'n': {
          // Toggle notes panel (preview questions only, not when Other is selected)
          if (
            hasPreviewOptions(currentQuestion) &&
            answers[currentQuestion.question] !== OTHER_SENTINEL
          ) {
            e.preventDefault();
            e.stopPropagation();
            toggleNotes(currentQuestion.question);
            // If opening notes, focus the textarea after render
            if (!expandedNotes[currentQuestion.question]) {
              setTimeout(() => {
                const textarea = panelRef.current?.querySelector(
                  '.question-notes textarea'
                ) as HTMLTextAreaElement | null;
                textarea?.focus();
              }, 0);
            }
          }
          break;
        }
        default:
          // Don't consume keys we don't handle -- let them bubble
          return;
      }
    },
    [
      currentQuestion,
      currentStep,
      focusedIndex,
      showConfirmDecline,
      isLastStep,
      enterPressedOnLast,
      expandedNotes,
      otherTexts,
      answers,
      questions,
      handleSubmit,
      selectFocusedOption,
      isQuestionAnswered,
      goNext,
      goBack,
      showToast,
      toggleNotes,
    ]
  );

  if (!currentQuestion) return null;

  return (
    <div
      className="question-panel"
      ref={panelRef}
      tabIndex={0}
      onKeyDown={handleKeyDown}
    >
      {/* Breadcrumb navigation: each question's header as a clickable tab */}
      {totalSteps > 1 && (
        <div className="question-breadcrumbs">
          <span className="question-step-counter" aria-label="Question progress">
            {currentStep + 1} of {totalSteps}
          </span>
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
          title="Escape to decline"
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
          {totalSteps > 1 && (
            <button
              className="question-btn question-btn--nav"
              onClick={goBack}
              disabled={isFirstStep || submitting}
              title={formatShortcut('Shift+Tab for previous')}
            >
              <ArrowLeft size={16} />
              Back
            </button>
          )}
          {!isLastStep ? (
            <button
              className="question-btn question-btn--next"
              onClick={goNext}
              disabled={submitting}
              title={formatShortcut('Tab for next')}
            >
              Next
              <ArrowRight size={16} />
            </button>
          ) : (
            <button
              className="question-btn question-btn--submit"
              onClick={handleSubmit}
              disabled={!allAnswered || submitting}
              title={
                !allAnswered
                  ? 'Answer all questions before submitting'
                  : formatShortcut('Ctrl+Enter to submit')
              }
            >
              <Check size={16} />
              {submitting ? 'Sending...' : 'Submit'}
            </button>
          )}
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
  otherInputRef: React.RefObject<HTMLTextAreaElement | null>;
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

  /** Render the "Other" element. In preview mode, it's a plain radio (no inline input). */
  const renderOtherOption = () => {
    const otherIndex = q.options.length;
    const isOtherFocused = focusedIndex === otherIndex;
    const otherSelected = isMulti
      ? multiSelected.has(OTHER_SENTINEL)
      : answer === OTHER_SENTINEL;

    // In preview mode: "Other" is a plain radio option (text input lives in the preview pane)
    if (isPreviewMode) {
      return (
        <div
          key="__other__"
          className={`question-option${otherSelected ? ' selected' : ''}${isOtherFocused ? ' focused' : ''}`}
          onClick={() => onSelect(q.question, OTHER_SENTINEL)}
          onMouseEnter={() => onFocusIndex(otherIndex)}
        >
          <input
            type="radio"
            name={q.question}
            checked={otherSelected}
            onChange={() => onSelect(q.question, OTHER_SENTINEL)}
            tabIndex={-1}
          />
          <span className="question-option-label">Other</span>
        </div>
      );
    }

    // Non-preview mode: "Other" with inline text input
    return (
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
          onClick={(e) => {
            e.stopPropagation();
            setTimeout(() => otherInputRef.current?.focus(), 0);
          }}
          tabIndex={-1}
        />
        <textarea
          ref={otherInputRef}
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
          rows={1}
          tabIndex={-1}
        />
      </div>
    );
  };

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
              onClick={(e) => e.stopPropagation()}
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

    return [...optionElements, renderOtherOption()];
  };

  return (
    <div className="question-item">
      <span className="question-header">{q.header}</span>
      <div className="question-text">
        <ReactMarkdown>{q.question}</ReactMarkdown>
      </div>

      {isPreviewMode ? (
        <div className="question-preview-layout">
          <div className="question-options">
            {renderOptions()}
            {answer !== OTHER_SENTINEL && (
              <span className="question-notes-hint">Press n to add notes</span>
            )}
          </div>
          <div
            className={`question-preview-pane${answer !== OTHER_SENTINEL && !activePreview ? ' question-preview-pane--empty' : ''}`}
          >
            {answer === OTHER_SENTINEL ? (
              <textarea
                className="question-preview-other-input"
                placeholder="Describe your preferred approach..."
                value={otherText}
                onChange={(e) => onOtherText(q.question, e.target.value)}
                autoFocus
              />
            ) : (
              activePreview || 'Select an option to preview'
            )}
          </div>
        </div>
      ) : (
        <div className="question-options">{renderOptions()}</div>
      )}

      {/* Notes only for single-select with previews, not when Other is selected */}
      {isPreviewMode && answer !== OTHER_SENTINEL && (
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
