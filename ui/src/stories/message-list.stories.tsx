import type { Story } from '@ladle/react';
import { MessageListFixture, getMessageListScenario, messageListScenarios } from '../fixtures/messageList';

const storyFor = (id: (typeof messageListScenarios)[number]['id']): Story => {
  const scenario = getMessageListScenario(id);
  return function MessageListStory() {
    return <MessageListFixture scenario={scenario} />;
  };
};

export const CompactLatestExpanded = storyFor('compact-latest-expanded');
CompactLatestExpanded.storyName = 'compact-latest-expanded';

export const CompactToolStrip = storyFor('compact-tool-strip');
CompactToolStrip.storyName = 'compact-tool-strip';
