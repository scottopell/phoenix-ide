// REQ-AUQ-011: answer Ember's open questions on Ember's page.
//
// Ember keeps working while a question is open, so the panel sits above the
// conversation instead of replacing the composer: the user can still message
// Ember, and answer whenever they are ready. One question shows at a time,
// oldest first, with a count of the rest.

import { useCallback } from 'react';
import { api } from '../api';
import type { CoordinatorQuestion } from '../api';
import { forgetCoordinatorQuestion, useCoordinatorQuestions } from '../coordinator/coordinatorQuestions';
import { useToast } from '../hooks/useToast';
import { QuestionPanel } from './QuestionPanel';
import { Toast } from './Toast';
import './CoordinatorQuestionDock.css';

function askedLabel(question: CoordinatorQuestion, now: number): string {
  const askedAt = Date.parse(question.asked_at);
  if (Number.isNaN(askedAt)) return 'Ember asked';
  const minutes = Math.max(0, Math.round((now - askedAt) / 60_000));
  if (minutes < 1) return 'Ember asked just now';
  if (minutes < 60) return `Ember asked ${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  return `Ember asked ${hours} h ago`;
}

export function CoordinatorQuestionDock() {
  const { questions } = useCoordinatorQuestions();
  const { toasts, dismissToast, showInfo } = useToast();
  const current = questions[0];
  const requestId = current?.request_id;
  const forgetCurrent = useCallback(() => {
    if (requestId) forgetCoordinatorQuestion(requestId);
  }, [requestId]);
  const submitAnswers = useCallback(
    (answers: Record<string, string>, annotations: Record<string, { notes?: string; preview?: string }> | undefined) => (
      requestId ? api.answerCoordinatorQuestion(requestId, answers, annotations) : Promise.resolve()
    ),
    [requestId],
  );
  const submitDismiss = useCallback(
    () => (requestId ? api.dismissCoordinatorQuestion(requestId) : Promise.resolve()),
    [requestId],
  );
  if (!current) return <Toast messages={toasts} onDismiss={dismissToast} />;
  const waiting = questions.length - 1;
  return (
    <section className="coordinator-question-dock" aria-label="Ember's questions">
      <p className="coordinator-question-dock__meta">
        <span>{askedLabel(current, Date.now())}. Ember keeps working while you decide.</span>
        {waiting > 0 && <span className="coordinator-question-dock__more">{waiting} more waiting</span>}
      </p>
      <QuestionPanel
        key={current.request_id}
        questions={current.questions}
        conversationId={current.coordinator_conversation_id}
        requestId={current.request_id}
        showToast={showInfo}
        submitAnswers={submitAnswers}
        submitDismiss={submitDismiss}
        dismissedMessage="Question dismissed. Ember will be told."
        dismissConfirmMessage="No answer will be sent. Ember is told you dismissed it and keeps working without it."
        onAnswered={forgetCurrent}
        onDismissed={forgetCurrent}
      />
      <Toast messages={toasts} onDismiss={dismissToast} />
    </section>
  );
}
