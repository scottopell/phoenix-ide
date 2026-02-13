// Shared types

export interface Breadcrumb {
  type: 'user' | 'llm' | 'tool' | 'subagents';
  label: string;
  toolId?: string | undefined;
  sequenceId?: number | undefined;
  preview?: string | undefined;
}

export interface AppState {
  conversations: import('./api').Conversation[];
  currentConversation: import('./api').Conversation | null;
  messages: import('./api').Message[];
  convState: string;
  stateData: import('./api').ConversationState | null;
  breadcrumbs: Breadcrumb[];
  agentWorking: boolean;
}
