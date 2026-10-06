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
export const TransportReconnecting = storyFor('transport-reconnecting');
TransportReconnecting.storyName = 'transport-reconnecting';
export const TransportDisconnected = storyFor('transport-disconnected');
TransportDisconnected.storyName = 'transport-disconnected';
export const OrdinaryReconnectingFrozen = storyFor('ordinary-reconnecting-frozen');
OrdinaryReconnectingFrozen.storyName = 'ordinary-reconnecting-frozen';
export const GlobalWatchdogStale = storyFor('global-watchdog-stale');
GlobalWatchdogStale.storyName = 'global-watchdog-stale';
