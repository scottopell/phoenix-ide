import type { Story } from '@ladle/react';
import { ProductConversationFixture, getProductConversationScenario } from '../fixtures/productConversation';
import type { EnrichedMessage } from '../generated/EnrichedMessage';

function reviewHistory(messages: EnrichedMessage[]): EnrichedMessage[] {
  const prompt = messages.find(message => message.message_type === 'user')!;
  const answer = messages.find(message => message.message_type === 'agent')!;
  const nextSequence = Math.max(...messages.map(message => message.sequence_id)) + 1;
  const turns = Array.from({ length: 32 }, (_, index) => [
    { ...prompt, message_id: `review-question-${index}`, sequence_id: nextSequence + index * 2,
      content: { text: `Check recovery step ${index + 1}.` } },
    { ...answer, message_id: `review-answer-${index}`, sequence_id: nextSequence + index * 2 + 1,
      content: [{ type: 'text' as const, text: `Recovery step ${index + 1}: preserve the completed result and the unfinished draft. Replay accepted events in order and reject stale completions.\n\nVerify that the result stays attached to its original conversation. Retrying must not duplicate an answer or move feedback to another source.` }] },
  ]).flat();
  return [...messages, ...turns, {
    ...prompt, message_id: 'review-tail-question', sequence_id: nextSequence + turns.length,
    content: { text: 'Summarize the final recovery check.' },
  }, {
    ...answer, message_id: 'review-tail-answer', sequence_id: nextSequence + turns.length + 1,
    content: [{ type: 'text', text: 'Recovery checks complete. Review this final result.' }],
  }];
}

// Both bases retain their real lazy-history adapter, cursor pages and source pins.
const ordinaryBase = getProductConversationScenario('ordinary-lazy-history');
const ordinary = {
  ...ordinaryBase,
  snapshot: {
    ...ordinaryBase.snapshot!,
    segments: ordinaryBase.snapshot!.segments.map(segment => segment.transcript_row_id === 'history-3'
      ? { ...segment, messages: reviewHistory(segment.messages) } : segment),
  },
};
const globalBase = getProductConversationScenario('global-lazy-history');
const global = {
  ...globalBase,
  snapshot: {
    ...globalBase.snapshot,
    segments: globalBase.snapshot.segments.map(segment => segment.transcript_row_id === 'history-3'
      ? { ...segment, messages: reviewHistory(segment.messages) } : segment),
  },
};

export const OrdinaryLongHistory: Story = () => <ProductConversationFixture scenario={ordinary} />;
OrdinaryLongHistory.storyName = 'ordinary-long-history';
export const GlobalLongHistory: Story = () => <ProductConversationFixture scenario={global} />;
GlobalLongHistory.storyName = 'global-long-history';
