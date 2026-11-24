import { useEffect, useId, useMemo, useRef, useState } from 'react';
import { api } from '../api';
import type { CoordinatorQuestion } from '../api';
import { forgetCoordinatorQuestion, useCoordinatorQuestions } from '../coordinator/coordinatorQuestions';
import { useToast } from '../hooks/useToast';
import { formatRelativeTime } from '../utils';
import { QuestionPanel } from './QuestionPanel';
import type { QuestionPanelActions } from './questionPanelActions';
import { Toast } from './Toast';
import './CoordinatorQuestionDock.css';

const MOBILE_QUESTION_QUERY = '(max-width: 768px)';

function coordinatorQuestionActions(requestId: string): QuestionPanelActions {
  return {
    submitAnswers: (answers, annotations) => api.answerCoordinatorQuestion(requestId, answers, annotations),
    dismiss: () => api.dismissCoordinatorQuestion(requestId),
    dismissedMessage: 'Question dismissed. Ember will be told.',
    dismissConfirmMessage: 'No answer will be sent. Ember is told you dismissed it and keeps working without it.',
  };
}

function useMobileQuestionLayout(): boolean {
  const [mobile, setMobile] = useState(() =>
    typeof window.matchMedia === 'function' && window.matchMedia(MOBILE_QUESTION_QUERY).matches,
  );

  useEffect(() => {
    if (typeof window.matchMedia !== 'function') return;
    const query = window.matchMedia(MOBILE_QUESTION_QUERY);
    const update = () => setMobile(query.matches);
    update();
    query.addEventListener('change', update);
    return () => query.removeEventListener('change', update);
  }, []);

  return mobile;
}

function setVisualViewportBounds(dialog: HTMLDialogElement): () => void {
  const viewport = window.visualViewport;
  const update = () => {
    dialog.style.setProperty('--coordinator-question-viewport-top', `${viewport?.offsetTop ?? 0}px`);
    dialog.style.setProperty('--coordinator-question-viewport-left', `${viewport?.offsetLeft ?? 0}px`);
    dialog.style.setProperty('--coordinator-question-viewport-width', `${viewport?.width ?? window.innerWidth}px`);
    dialog.style.setProperty('--coordinator-question-viewport-height', `${viewport?.height ?? window.innerHeight}px`);
  };

  update();
  viewport?.addEventListener('resize', update);
  viewport?.addEventListener('scroll', update);
  window.addEventListener('resize', update);

  return () => {
    viewport?.removeEventListener('resize', update);
    viewport?.removeEventListener('scroll', update);
    window.removeEventListener('resize', update);
  };
}

function CoordinatorQuestionCard({
  question,
  waiting,
  showToast,
}: {
  question: CoordinatorQuestion;
  waiting: number;
  showToast: (message: string, duration?: number) => void;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const launcherRef = useRef<HTMLButtonElement>(null);
  const titleId = useId();
  const descriptionId = useId();
  const mobile = useMobileQuestionLayout();
  const [expanded, setExpanded] = useState(true);
  const actions = useMemo(() => coordinatorQuestionActions(question.request_id), [question.request_id]);
  const forget = () => forgetCoordinatorQuestion(question.request_id);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;

    if (mobile && !expanded) {
      if (dialog.open) dialog.close();
      return;
    }

    const clearViewportBounds = mobile ? setVisualViewportBounds(dialog) : undefined;
    if (mobile) dialog.showModal();
    else dialog.open = true;

    dialog.querySelector<HTMLElement>('.confirm-dialog .btn-secondary')?.focus({ preventScroll: true });

    const focusFrame = mobile
      ? requestAnimationFrame(() => {
          if (!dialog.querySelector('.confirm-dialog')) {
            dialog.querySelector<HTMLElement>('.question-panel')?.focus({ preventScroll: true });
          }
        })
      : undefined;

    return () => {
      if (focusFrame !== undefined) cancelAnimationFrame(focusFrame);
      clearViewportBounds?.();
      if (dialog.open) dialog.close();
    };
  }, [expanded, mobile]);

  useEffect(() => {
    if (!mobile || expanded) return;
    const frame = requestAnimationFrame(() => launcherRef.current?.focus({ preventScroll: true }));
    return () => cancelAnimationFrame(frame);
  }, [expanded, mobile]);

  const returnToConversation = () => setExpanded(false);

  return (
    <section className="coordinator-question-dock" aria-label="Ember's questions">
      {mobile && !expanded && (
        <button
          ref={launcherRef}
          type="button"
          className="coordinator-question-dock__launcher"
          onClick={() => setExpanded(true)}
          aria-label="Answer Ember’s question"
        >
          <span>Answer Ember’s question</span>
          {waiting > 0 && <span className="coordinator-question-dock__launcher-count">+{waiting}</span>}
        </button>
      )}

      <dialog
        ref={dialogRef}
        className="coordinator-question-dock__dialog"
        aria-labelledby={titleId}
        aria-describedby={descriptionId}
        onCancel={(event) => {
          event.preventDefault();
          const confirmCancel = event.currentTarget.querySelector<HTMLButtonElement>('.confirm-dialog .btn-secondary');
          if (confirmCancel) {
            confirmCancel.click();
            return;
          }
          if (mobile) returnToConversation();
        }}
        onKeyDownCapture={(event) => {
          const target = event.target;
          const editing = target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement
            || (target instanceof HTMLElement && target.isContentEditable);
          if (event.key === '?' && !editing) {
            event.preventDefault();
            event.stopPropagation();
            if (mobile) returnToConversation();
            window.dispatchEvent(new CustomEvent('toggle-shortcut-help'));
            return;
          }
          if (!mobile || event.key !== 'Escape') return;
          if (event.currentTarget.querySelector('.confirm-dialog')) {
            event.preventDefault();
            return;
          }
          if (event.target instanceof HTMLInputElement || event.target instanceof HTMLTextAreaElement) return;
          event.preventDefault();
          event.stopPropagation();
          returnToConversation();
        }}
        onMouseDown={(event) => {
          if (mobile && event.target === event.currentTarget) returnToConversation();
        }}
      >
        <div className="coordinator-question-dock__surface">
          <header className="coordinator-question-dock__mobile-header">
            <div>
              <h2 id={titleId}>Ember has a question</h2>
              <p>Answer now or return without losing your choices.</p>
            </div>
            <button
              type="button"
              className="coordinator-question-dock__return"
              onClick={returnToConversation}
              aria-label="Return to conversation"
            >
              Return to conversation
            </button>
          </header>

          <div className="coordinator-question-dock__scroller">
            <p id={descriptionId} className="coordinator-question-dock__meta">
              <span>Ember asked {formatRelativeTime(question.asked_at)}. Ember keeps working while you decide.</span>
              {waiting > 0 && <span className="coordinator-question-dock__more">{waiting} more waiting</span>}
            </p>
            <QuestionPanel
              questions={question.questions}
              actions={actions}
              showToast={showToast}
              onAnswered={forget}
              onDismissed={forget}
              active={!mobile || expanded}
            />
          </div>
        </div>
      </dialog>
    </section>
  );
}

export function CoordinatorQuestionDock() {
  const questions = useCoordinatorQuestions();
  const { toasts, dismissToast, showInfo } = useToast();
  const current = questions[0];
  return (
    <>
      {current && (
        <CoordinatorQuestionCard
          key={current.request_id}
          question={current}
          waiting={questions.length - 1}
          showToast={showInfo}
        />
      )}
      <Toast messages={toasts} onDismiss={dismissToast} />
    </>
  );
}
