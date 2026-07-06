import type { ConversationState, Message } from '../../api';

export interface MessageListFixtureData {
  conversationId: string;
  slug: string;
  theme: 'light' | 'dark';
  messages: Message[];
  pendingMessages: never[];
  convState: ConversationState;
}

export interface MessageListScenario {
  id: string;
  title: string;
  description: string;
  theme: 'light' | 'dark';
}
