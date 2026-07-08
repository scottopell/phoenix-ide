import type { Story } from '@ladle/react';
import { SidebarFixture, sidebarScenarios } from '../fixtures/sidebar';
import type { SidebarScenarioId } from '../fixtures/sidebar';

const storyFor = (id: SidebarScenarioId): Story => {
  const scenario = sidebarScenarios.find((item) => item.id === id);
  if (!scenario) throw new Error(`Unknown sidebar scenario: ${id}`);
  return function SidebarStory() {
    return <SidebarFixture scenario={scenario} />;
  };
};

export const ExpandedAllActive = storyFor('expanded-all-active');
ExpandedAllActive.storyName = 'expanded-all-active';

export const ExpandedProjectArchived = storyFor('expanded-project-archived');
ExpandedProjectArchived.storyName = 'expanded-project-archived';

export const ExpandedEmptyProject = storyFor('expanded-empty-project');
ExpandedEmptyProject.storyName = 'expanded-empty-project';

export const CollapsedOverflow = storyFor('collapsed-overflow');
CollapsedOverflow.storyName = 'collapsed-overflow';
