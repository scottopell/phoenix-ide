import type { Story } from '@ladle/react';
import { TaskApprovalFixture, taskApprovalScenarios } from '../fixtures/taskApproval';

const storyFor = (id: string): Story => {
  const scenario = taskApprovalScenarios.find((item) => item.id === id);
  if (!scenario) throw new Error(`Unknown task approval scenario: ${id}`);
  return function TaskApprovalStory() {
    return <TaskApprovalFixture scenario={scenario} />;
  };
};

export const MobileDark = storyFor('mobile-dark');
MobileDark.storyName = 'mobile-dark';

export const MobileLight = storyFor('mobile-light');
MobileLight.storyName = 'mobile-light';
