import type { Story } from '@ladle/react';
import { MobileConversationListFixture, mobileConversationListScenarios } from '../fixtures/mobileConversationList';
import type { MobileConversationListScenarioId } from '../fixtures/mobileConversationList';

const storyFor = (id: MobileConversationListScenarioId): Story => {
  const scenario = mobileConversationListScenarios.find((item) => item.id === id);
  if (!scenario) throw new Error(`Unknown mobile conversation list scenario: ${id}`);
  return function MobileConversationListStory() {
    return <MobileConversationListFixture scenario={scenario} />;
  };
};

export const ActiveOverviewDark = storyFor('active-overview-dark');
ActiveOverviewDark.storyName = 'active-overview-dark';

export const ActiveOverviewLight = storyFor('active-overview-light');
ActiveOverviewLight.storyName = 'active-overview-light';

export const ChainsDark = storyFor('chains-dark');
ChainsDark.storyName = 'chains-dark';

export const NamingContextDark = storyFor('naming-context-dark');
NamingContextDark.storyName = 'naming-context-dark';

export const ArchivedDark = storyFor('archived-dark');
ArchivedDark.storyName = 'archived-dark';
