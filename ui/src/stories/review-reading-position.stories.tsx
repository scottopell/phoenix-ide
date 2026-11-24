import type { Story } from '@ladle/react';
import { ProductConversationFixture, getProductConversationScenario } from '../fixtures/productConversation';
import type { EnrichedMessage } from '../generated/EnrichedMessage';
import { useConversationAtom } from '../conversation/useConversationAtom';

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

// Long, image-free prose for distinguishing viewer-content scrolling from the background reader.
function longReviewMessages(messages: EnrichedMessage[]): EnrichedMessage[] {
  return messages.map(message => message.message_type === 'agent' ? {
    ...message,
    content: [{ type: 'text' as const, text: Array.from({ length: 36 }, (_, index) =>
      `Review paragraph ${index + 1}: preserve the visible reading position while this image-free message is settled. Notes remain linked to their original source; opening an editor must preserve intentional keyboard navigation.`).join('\n\n') }],
  } : message);
}
const idleOrdinary = { ...ordinary, snapshot: { ...ordinary.snapshot, segments: ordinary.snapshot.segments.map(segment => ({ ...segment, messages: longReviewMessages(segment.messages) })) } };
const idleGlobal = { ...global, snapshot: { ...global.snapshot, segments: global.snapshot.segments.map(segment => ({ ...segment, messages: longReviewMessages(segment.messages) })) } };
function projectionMessages(messages: EnrichedMessage[], mermaid: boolean): EnrichedMessage[] {
  return longReviewMessages(messages).map(message => message.message_id === 'history-answer-3' && mermaid ? {
    ...message,
    content: [{ type: 'text' as const, text: `\`\`\`mermaid\nflowchart TD\n  A["Reviewed message"] --> B["Stable diagram"] --> C["No new input"] --> D["Same text anchor below"]\n\`\`\`\n\n${(message.content as { type: 'text'; text: string }[])[0]!.text}` }],
  } : message);
}
function projectionScenario(mermaid: boolean) {
  return {
    ...idleOrdinary,
    reviewProjectionQa: true,
    snapshot: {
      ...idleOrdinary.snapshot,
      segments: idleOrdinary.snapshot.segments.map(segment => ({ ...segment, messages: projectionMessages(segment.messages, mermaid) })),
    },
  };
}
const projectionFixture = projectionScenario(false);
const mermaidProjectionFixture = projectionScenario(true);
export const OrdinaryIdleReview: Story = () => <ProductConversationFixture scenario={idleOrdinary} />;
OrdinaryIdleReview.storyName = 'ordinary-idle-review';
export const GlobalIdleReview: Story = () => <ProductConversationFixture scenario={idleGlobal} />;
GlobalIdleReview.storyName = 'global-idle-review';

function ReviewProjectionProbe() {
  const [atom] = useConversationAtom('history-3');
  return <span hidden data-review-projection-probe data-title={atom.conversation?.title ?? ''}
    data-reviewed-content={JSON.stringify(atom.messages.find(message => message.message_id === 'history-answer-3')?.content)} />;
}
export const OverlayProjectionMermaid: Story = () => <ProductConversationFixture scenario={mermaidProjectionFixture}><ReviewProjectionProbe /></ProductConversationFixture>;
OverlayProjectionMermaid.storyName = 'overlay-projection-mermaid';
export const OverlayProjectionText: Story = () => <ProductConversationFixture scenario={projectionFixture}><ReviewProjectionProbe /></ProductConversationFixture>;
OverlayProjectionText.storyName = 'overlay-projection-text';
