import type { Story } from '@ladle/react';
import {
  getNewConversationScenario,
  NewConversationFixture,
  type NewConversationScenarioId,
} from '../fixtures/newConversation';

const storyFor = (id: NewConversationScenarioId): Story => {
  const scenario = getNewConversationScenario(id);
  return function NewConversationStory() {
    return <NewConversationFixture scenario={scenario} />;
  };
};

export const ReadyGitProject = storyFor('ready-git-project');
ReadyGitProject.storyName = 'ready-git-project';
