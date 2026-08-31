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

export const DesktopOpenMultiSegmentQaWork = storyFor('desktop-open-multi-segment-qa-work');
DesktopOpenMultiSegmentQaWork.storyName = 'desktop-open-multi-segment-qa-work';

export const MobileOpen = storyFor('mobile-open');
MobileOpen.storyName = 'mobile-open';

export const DesktopHistoryReadOnly = storyFor('desktop-history-read-only');
DesktopHistoryReadOnly.storyName = 'desktop-history-read-only';

export const MobileHistoryReadOnly = storyFor('mobile-history-read-only');
MobileHistoryReadOnly.storyName = 'mobile-history-read-only';

export const Loading = storyFor('loading');
Loading.storyName = 'loading';

export const Error = storyFor('error');
Error.storyName = 'error';

export const LongHistory110Messages = storyFor('long-history-110-messages');
LongHistory110Messages.storyName = 'long-history-110-messages';
