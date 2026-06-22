import type { Story } from '@ladle/react';
import { ConversationPanelFixture, conversationPanelScenarios } from '../fixtures/conversationPanel';

const storyFor = (id: string): Story => {
  const scenario = conversationPanelScenarios.find((item) => item.id === id);
  if (!scenario) throw new Error(`Unknown conversation panel scenario: ${id}`);
  return function ConversationPanelStory() {
    return <ConversationPanelFixture scenario={scenario} />;
  };
};

export const ExpandedDark = storyFor('expanded-dark');
ExpandedDark.storyName = 'expanded-dark';

export const ExpandedLight = storyFor('expanded-light');
ExpandedLight.storyName = 'expanded-light';

export const CollapsedDark = storyFor('collapsed-dark');
CollapsedDark.storyName = 'collapsed-dark';

export const NarrowDark = storyFor('narrow-dark');
NarrowDark.storyName = 'narrow-dark';

export const ArchivedDark = storyFor('archived-dark');
ArchivedDark.storyName = 'archived-dark';
