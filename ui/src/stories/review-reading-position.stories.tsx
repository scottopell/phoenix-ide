import type { Story } from '@ladle/react';
import { ProductConversationFixture, getProductConversationScenario } from '../fixtures/productConversation';
import { makeVirtualizedReactionScenario } from '../fixtures/productConversation/scenarios';

// Reuse the Global lazy-history adapter, topology and cursor pages; extend its
// newest member with the ordinary reaction fixture's long prose history.
const base = getProductConversationScenario('global-lazy-history');
const prose = makeVirtualizedReactionScenario().snapshot!.segments[0]!.messages
  .filter(message => message.message_id.startsWith('recovery-'));
const scenario = {
  ...base,
  snapshot: {
    ...base.snapshot,
    segments: base.snapshot.segments.map(segment => segment.transcript_row_id === 'history-3'
      ? { ...segment, messages: [...segment.messages, ...prose.map((message, index) => ({
        ...message, conversation_id: 'history-3', message_id: `review-global-${index}`, sequence_id: index + 3,
      }))] }
      : segment),
  },
};

export const GlobalLongHistory: Story = () => <ProductConversationFixture scenario={scenario} />;
GlobalLongHistory.storyName = 'global-long-history';
