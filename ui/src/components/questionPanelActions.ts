import { api } from '../api';

export type QuestionAnnotations = Record<string, { notes?: string; preview?: string }>;

/** What submitting or dismissing the panel does, and how it is worded. */
export interface QuestionPanelActions {
  submitAnswers: (answers: Record<string, string>, annotations: QuestionAnnotations | undefined) => Promise<unknown>;
  dismiss: () => Promise<unknown>;
  /** Toast after a dismissal. */
  dismissedMessage: string;
  /** Body of the dismiss confirmation. */
  dismissConfirmMessage: string;
}

/** Answers a conversation blocked in `awaiting_user_response`. */
export function blockingQuestionActions(conversationId: string, requestId: string | undefined): QuestionPanelActions {
  return {
    submitAnswers: (answers, annotations) => api.respondToQuestion(conversationId, requestId, answers, annotations),
    dismiss: () => api.dismissQuestion(conversationId, requestId),
    dismissedMessage: 'Question dismissed. Type a message to continue.',
    dismissConfirmMessage: 'The question panel will close. No answer will be sent, and the agent will not continue until you type a message.',
  };
}
