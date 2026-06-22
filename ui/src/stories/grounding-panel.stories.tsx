import type { Story } from '@ladle/react';
import { MemoryRouter } from 'react-router-dom';
import { ConversationProvider } from '../conversation';
import { GroundingPanelFixture, groundingPanelScenarios } from '../fixtures/groundingPanel';

const storyFor = (id: string): Story => {
  const scenario = groundingPanelScenarios.find((item) => item.id === id);
  if (!scenario) throw new Error(`Unknown grounding panel scenario: ${id}`);
  return function GroundingPanelStory() {
    return (
      <MemoryRouter initialEntries={['/c/grounding-panel-fixture']}>
        <ConversationProvider>
        <GroundingPanelFixture scenario={scenario} showToolbar={false} />
      </ConversationProvider>
      </MemoryRouter>
    );
  };
};

export const FullDark = storyFor('full-dark');
FullDark.storyName = 'full-dark';

export const FullLight = storyFor('full-light');
FullLight.storyName = 'full-light';

export const EmptyDark = storyFor('empty-dark');
EmptyDark.storyName = 'empty-dark';

export const ErrorsDark = storyFor('errors-dark');
ErrorsDark.storyName = 'errors-dark';

export const CollapsedDark = storyFor('collapsed-dark');
CollapsedDark.storyName = 'collapsed-dark';

export const WorkDark = storyFor('work-dark');
WorkDark.storyName = 'work-dark';

export const WorkLight = storyFor('work-light');
WorkLight.storyName = 'work-light';

export const NarrowDark = storyFor('narrow-dark');
NarrowDark.storyName = 'narrow-dark';

export const SkillDetailDark = storyFor('skill-detail-dark');
SkillDetailDark.storyName = 'skill-detail-dark';

export const TaskDetailDark = storyFor('task-detail-dark');
TaskDetailDark.storyName = 'task-detail-dark';
