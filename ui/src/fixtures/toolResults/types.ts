import type { ConversationState, Message } from '../../api';

export type ToolResultsDensity = 'full' | 'compact';
export type ToolResultsScenarioFamily =
  | 'shell'
  | 'execution'
  | 'discovery'
  | 'media'
  | 'profiling'
  | 'subagents';

export interface ToolResultsFixtureData {
  conversationId: string;
  slug: string;
  theme: 'dark';
  density: ToolResultsDensity;
  filePathRootDir: string;
  workScopeKey: string;
  messages: Message[];
  pendingMessages: never[];
  convState: ConversationState;
}

export interface ToolResultsScenario {
  id: string;
  title: string;
  description: string;
  density: ToolResultsDensity;
  family: ToolResultsScenarioFamily;
}
