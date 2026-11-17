// REQ-AUQ-010..012: questions Ember posted without stopping its own work.
//
// One shared snapshot, refreshed by a poll mounted once in the app shell.
// Ember's page renders the cards, the Ember entry shows that something waits,
// and the notification feed announces questions that appear while the app is
// open.

import { useCallback, useSyncExternalStore } from 'react';
import { api } from '../api';
import type { CoordinatorQuestion } from '../api';
import { notifyCoordinatorQuestionFeed } from '../notifications';
import { useReminderRefresh } from '../reminders/useReminderRefresh';

export interface CoordinatorQuestionsSnapshot {
  questions: readonly CoordinatorQuestion[];
  /** True once the server has answered at least once. */
  loaded: boolean;
}

let snapshot: CoordinatorQuestionsSnapshot = { questions: [], loaded: false };
const listeners = new Set<() => void>();

function publish(next: CoordinatorQuestionsSnapshot): void {
  snapshot = next;
  listeners.forEach((listener) => listener());
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function getSnapshot(): CoordinatorQuestionsSnapshot {
  return snapshot;
}

/** Ember's open questions, oldest first. */
export function useCoordinatorQuestions(): CoordinatorQuestionsSnapshot {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

/** Drop a question the user just answered or dismissed, before the next poll. */
export function forgetCoordinatorQuestion(requestId: string): void {
  const questions = snapshot.questions.filter((question) => question.request_id !== requestId);
  if (questions.length !== snapshot.questions.length) publish({ ...snapshot, questions });
}

/** Headline for a notification: the first question, with a count of the rest. */
export function coordinatorQuestionHeadline(question: CoordinatorQuestion): string {
  const first = question.questions[0]?.question ?? 'Ember has a question';
  const more = question.questions.length - 1;
  return more > 0 ? `${first} (+${more} more)` : first;
}

export function receiveCoordinatorQuestions(questions: readonly CoordinatorQuestion[]): void {
  publish({ questions, loaded: true });
  notifyCoordinatorQuestionFeed(questions.map((question) => ({
    requestId: question.request_id,
    coordinatorConversationId: question.coordinator_conversation_id,
    text: coordinatorQuestionHeadline(question),
  })));
}

const ignoreFailure = () => undefined;

/** Mount once in the app shell: keeps the snapshot and notifications current. */
export function useCoordinatorQuestionFeed(): void {
  const load = useCallback((signal: AbortSignal) => api.listCoordinatorQuestions(signal), []);
  useReminderRefresh(load, receiveCoordinatorQuestions, ignoreFailure);
}

export function resetCoordinatorQuestionsForTest(): void {
  listeners.clear();
  snapshot = { questions: [], loaded: false };
}
