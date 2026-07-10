import type { Story } from '@ladle/react';
import { getToolResultsScenario, toolResultsScenarios, ToolResultsFixture } from '../fixtures/toolResults';

const storyFor = (id: (typeof toolResultsScenarios)[number]['id']): Story => {
  const scenario = getToolResultsScenario(id);
  return function ToolResultsStory() {
    return <ToolResultsFixture scenario={scenario} />;
  };
};

export const ShellFull = storyFor('shell-full');
ShellFull.storyName = 'shell-full';

export const ShellCompact = storyFor('shell-compact');
ShellCompact.storyName = 'shell-compact';

export const ExecutionFull = storyFor('execution-full');
ExecutionFull.storyName = 'execution-full';

export const ExecutionCompact = storyFor('execution-compact');
ExecutionCompact.storyName = 'execution-compact';

export const DiscoveryFull = storyFor('discovery-full');
DiscoveryFull.storyName = 'discovery-full';

export const DiscoveryCompact = storyFor('discovery-compact');
DiscoveryCompact.storyName = 'discovery-compact';

export const MediaFull = storyFor('media-full');
MediaFull.storyName = 'media-full';

export const MediaCompact = storyFor('media-compact');
MediaCompact.storyName = 'media-compact';

export const ProfilingFull = storyFor('profiling-full');
ProfilingFull.storyName = 'profiling-full';

export const ProfilingCompact = storyFor('profiling-compact');
ProfilingCompact.storyName = 'profiling-compact';

export const SubagentsFull = storyFor('subagents-full');
SubagentsFull.storyName = 'subagents-full';

export const SubagentsCompact = storyFor('subagents-compact');
SubagentsCompact.storyName = 'subagents-compact';
