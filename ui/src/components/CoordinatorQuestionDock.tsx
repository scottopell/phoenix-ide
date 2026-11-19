// REQ-AUQ-011: answer Ember's open questions on Ember's page.
//
// Ember keeps working while a question is open, so the panel sits above the
// conversation instead of replacing the composer: the user can still message
// Ember, and answer whenever they are ready. One question shows at a time,
// oldest first, with a count of the rest.

import { api } from '../api';
import type { CoordinatorQuestion } from '../api';
import { forgetCoordinatorQuestion, useCoordinatorQuestions } from '../coordinator/coordinatorQuestions';
import { useToast } from '../hooks/useToast';
import { formatRelativeTime } from '../utils';
import { QuestionPanel } from './QuestionPanel';
import type { QuestionPanelActions } from './questionPanelActions';
import { Toast } from './Toast';
import './CoordinatorQuestionDock.css';

function coordinatorQuestionActions(requestId: string): QuestionPanelActions {
  return {
    submitAnswers: (answers, annotations) => api.answerCoordinatorQuestion(requestId, answers, annotations),
    dismiss: () => api.dismissCoordinatorQuestion(requestId),
    dismissedMessage: 'Question dismissed. Ember will be told.',
    dismissConfirmMessage: 'No answer will be sent. Ember is told you dismissed it and keeps working without it.',
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
  const forget = () => forgetCoordinatorQuestion(question.request_id);
  return (
    <section className="coordinator-question-dock" aria-label="Ember's questions">
      <p className="coordinator-question-dock__meta">
        <span>Ember asked {formatRelativeTime(question.asked_at)}. Ember keeps working while you decide.</span>
        {waiting > 0 && <span className="coordinator-question-dock__more">{waiting} more waiting</span>}
      </p>
      <QuestionPanel
        questions={question.questions}
        actions={coordinatorQuestionActions(question.request_id)}
        showToast={showToast}
        onAnswered={forget}
        onDismissed={forget}
      />
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
