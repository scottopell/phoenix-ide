import type { Story } from '@ladle/react';
import {
  DesktopMultiPrConversationFixture,
  type DesktopMultiPrScenario,
} from '../fixtures/desktopMultiPrConversation/renderFixture';

const storyFor = (scenario: DesktopMultiPrScenario): Story => function DesktopMultiPrConversationStory() {
  return <DesktopMultiPrConversationFixture scenario={scenario} />;
};

export const CollapsedTwoOpen = storyFor('collapsed-two-open');
CollapsedTwoOpen.storyName = 'collapsed-two-open';

export const ExpandedActiveFeedback = storyFor('expanded-active-feedback');
ExpandedActiveFeedback.storyName = 'expanded-active-feedback';

export const AmbiguousTwoOpen = storyFor('ambiguous-two-open');
AmbiguousTwoOpen.storyName = 'ambiguous-two-open';
