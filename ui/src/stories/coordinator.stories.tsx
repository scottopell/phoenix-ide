import type { Story } from '@ladle/react';
import { CoordinatorFixture, coordinatorScenarios } from '../fixtures/coordinator';
import type { CoordinatorScenarioId } from '../fixtures/coordinator';

const storyFor = (id: CoordinatorScenarioId): Story => {
  const scenario = coordinatorScenarios.find((item) => item.id === id);
  if (!scenario) throw new Error(`Unknown Coordinator scenario: ${id}`);
  return function CoordinatorStory() { return <CoordinatorFixture scenario={scenario} />; };
};

export const ConversationIdle = storyFor('conversation-idle');
ConversationIdle.storyName = 'conversation-idle';
export const ConversationWorking = storyFor('conversation-working');
ConversationWorking.storyName = 'conversation-working';
export const FleetCompact = storyFor('fleet-compact');
FleetCompact.storyName = 'fleet-compact';
export const FleetExpanded = storyFor('fleet-expanded');
FleetExpanded.storyName = 'fleet-expanded';
export const FleetError = storyFor('fleet-error');
FleetError.storyName = 'fleet-error';
