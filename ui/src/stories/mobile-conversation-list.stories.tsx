import type { Story } from '@ladle/react';
import { MobileConversationListFixture, mobileConversationListScenarios } from '../fixtures/mobileConversationList';

const storyFor = (id: string): Story => {
  const scenario = mobileConversationListScenarios.find((item) => item.id === id);
  if (!scenario) throw new Error(`Unknown mobile conversation list scenario: ${id}`);
  return function MobileConversationListStory() {
    return <MobileConversationListFixture scenario={scenario} />;
  };
};

export const ActiveDark = storyFor('active-dark');
ActiveDark.storyName = 'active-dark';

export const ActiveLight = storyFor('active-light');
ActiveLight.storyName = 'active-light';

export const ArchivedDark = storyFor('archived-dark');
ArchivedDark.storyName = 'archived-dark';
