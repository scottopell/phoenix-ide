import type { Story } from '@ladle/react';
import {
  getMobileMultiPrConversationScenario,
  MobileMultiPrConversationFixture,
} from '../fixtures/mobileMultiPrConversation';

const storyFor = (id: string): Story => {
  const scenario = getMobileMultiPrConversationScenario(id);
  return function MobileMultiPrConversationStory() {
    return <MobileMultiPrConversationFixture scenario={scenario} />;
  };
};

export const Collapsed = storyFor('collapsed');
Collapsed.storyName = 'collapsed';

export const Expanded = storyFor('expanded');
Expanded.storyName = 'expanded';

export const ChooserOpen = storyFor('chooser-open');
ChooserOpen.storyName = 'chooser-open';

export const ActivePrActions = storyFor('active-pr-actions');
ActivePrActions.storyName = 'active-pr-actions';
