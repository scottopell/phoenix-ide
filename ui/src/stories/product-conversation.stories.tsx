import type { Story } from '@ladle/react';
import {
  getProductConversationScenario,
  ProductConversationFixture,
  type ProductConversationScenarioId,
} from '../fixtures/productConversation';

const storyFor = (id: ProductConversationScenarioId): Story => {
  const scenario = getProductConversationScenario(id);
  return function ProductConversationStory() {
    return <ProductConversationFixture scenario={scenario} />;
  };
};

export const DesktopMultiSegmentQaWork = storyFor('desktop-multi-segment-qa-work');
DesktopMultiSegmentQaWork.storyName = 'desktop-multi-segment-qa-work';

export const MobileOpen = storyFor('mobile-open');
MobileOpen.storyName = 'mobile-open';

export const HistoryReadOnly = storyFor('history-read-only');
HistoryReadOnly.storyName = 'history-read-only';

export const Loading = storyFor('loading');
Loading.storyName = 'loading';

export const Error = storyFor('error');
Error.storyName = 'error';

export const LongHistory110Messages = storyFor('long-history-110-messages');
LongHistory110Messages.storyName = 'long-history-110-messages';
