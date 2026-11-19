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
import { usePolledRefresh } from '../hooks/usePolledRefresh';

let questions: readonly CoordinatorQuestion[] = [];
const listeners = new Set<() => void>();

function publish(next: readonly CoordinatorQuestion[]): void {
  questions = next;
  listeners.forEach((listener) => listener());
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

const getQuestions = () => questions;
const getCount = () => questions.length;

/** Ember's open questions, oldest first. */
export function useCoordinatorQuestions(): readonly CoordinatorQuestion[] {
  return useSyncExternalStore(subscribe, getQuestions, getQuestions);
}

/** How many questions Ember is waiting on; re-renders only when it changes. */
export function useCoordinatorQuestionCount(): number {
  return useSyncExternalStore(subscribe, getCount, getCount);
}

/** Drop a question the user just answered or dismissed, before the next poll. */
export function forgetCoordinatorQuestion(requestId: string): void {
  const next = questions.filter((question) => question.request_id !== requestId);
  if (next.length !== questions.length) publish(next);
}

/** Posted questions never change, so the same request ids mean the same list. */
function sameQuestions(a: readonly CoordinatorQuestion[], b: readonly CoordinatorQuestion[]): boolean {
  return a.length === b.length && a.every((question, index) => question.request_id === b[index]?.request_id);
}

/** Headline for a notification: the first question, with a count of the rest. */
function coordinatorQuestionHeadline(question: CoordinatorQuestion): string {
  const first = question.questions[0]?.question ?? 'Ember has a question';
  const more = question.questions.length - 1;
  return more > 0 ? `${first} (+${more} more)` : first;
}

export function receiveCoordinatorQuestions(received: readonly CoordinatorQuestion[]): void {
  if (!sameQuestions(questions, received)) publish(received);
  notifyCoordinatorQuestionFeed(received.map((question) => ({
    requestId: question.request_id,
    coordinatorProductConversationId: question.coordinator_product_conversation_id,
    text: coordinatorQuestionHeadline(question),
  })));
}

const ignoreFailure = () => undefined;

/** Mount once in the app shell: keeps the snapshot and notifications current. */
export function useCoordinatorQuestionFeed(): void {
  const load = useCallback((signal: AbortSignal) => api.listCoordinatorQuestions(signal), []);
  usePolledRefresh(load, receiveCoordinatorQuestions, ignoreFailure);
}

export function resetCoordinatorQuestionsForTest(): void {
  listeners.clear();
  questions = [];
}
