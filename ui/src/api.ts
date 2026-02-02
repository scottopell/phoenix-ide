// Phoenix API Client

export interface Conversation {
  id: string;
  slug: string;
  cwd: string;
  created_at: string;
  updated_at: string;
  state?: ConversationState;
}

export interface ConversationState {
  type: string;
  attempt?: number;
  current_tool?: ToolCall;
  remaining_tools?: ToolCall[];
  pending_ids?: string[];
  completed_results?: unknown[];
  message?: string;
}

export interface ToolCall {
  id: string;
  input: { _tool?: string; [key: string]: unknown };
}

export interface Message {
  id: number;
  sequence_id: number;
  conversation_id: string;
  message_type: 'user' | 'agent' | 'tool';
  type?: string; // legacy
  content: MessageContent;
  usage_data?: UsageData;
  created_at: string;
}

export type MessageContent = 
  | { text: string; images?: ImageData[] }  // user message
  | ContentBlock[]  // agent message
  | ToolResultContent;  // tool result

export interface ContentBlock {
  type: 'text' | 'tool_use';
  text?: string;
  id?: string;
  name?: string;
  input?: Record<string, unknown>;
}

export interface ToolResultContent {
  tool_use_id: string;
  content?: string;
  result?: string;
  error?: string;
  is_error?: boolean;
}

export interface ImageData {
  data: string;
  media_type: string;
}

export interface UsageData {
  input_tokens: number;
  output_tokens: number;
  cache_creation_input_tokens?: number;
  cache_read_input_tokens?: number;
}

export interface SseInitData {
  conversation: Conversation;
  messages: Message[];
  agent_working: boolean;
  last_sequence_id: number;
}

export interface SseMessageData {
  message: Message;
}

export interface SseStateChangeData {
  state: ConversationState;
}

export type SseEventType = 'init' | 'message' | 'state_change' | 'agent_done' | 'disconnected';
export type SseEventData = SseInitData | SseMessageData | SseStateChangeData | Record<string, never>;

export const api = {
  async listConversations(): Promise<Conversation[]> {
    const resp = await fetch('/api/conversations');
    if (!resp.ok) throw new Error('Failed to list conversations');
    return (await resp.json()).conversations;
  },

  async createConversation(cwd: string): Promise<Conversation> {
    const resp = await fetch('/api/conversations/new', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ cwd }),
    });
    if (!resp.ok) {
      const err = await resp.json();
      throw new Error(err.error || 'Failed to create conversation');
    }
    return (await resp.json()).conversation;
  },

  async getConversationBySlug(slug: string): Promise<{ conversation: Conversation; messages: Message[]; agent_working: boolean }> {
    const resp = await fetch(`/api/conversations/by-slug/${encodeURIComponent(slug)}`);
    if (!resp.ok) {
      if (resp.status === 404) throw new Error('Conversation not found');
      throw new Error('Failed to get conversation');
    }
    return resp.json();
  },

  async sendMessage(convId: string, text: string, images: ImageData[] = []): Promise<{ queued: boolean }> {
    const resp = await fetch(`/api/conversations/${convId}/chat`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ text, images }),
    });
    if (!resp.ok) throw new Error('Failed to send message');
    return resp.json();
  },

  async validateCwd(path: string): Promise<{ valid: boolean; error?: string }> {
    const resp = await fetch(`/api/validate-cwd?path=${encodeURIComponent(path)}`);
    return resp.json();
  },

  async cancelConversation(convId: string): Promise<{ ok: boolean }> {
    const resp = await fetch(`/api/conversations/${convId}/cancel`, {
      method: 'POST',
    });
    if (!resp.ok) throw new Error('Failed to cancel');
    return resp.json();
  },

  streamConversation(
    convId: string,
    onEvent: (eventType: SseEventType, data: SseEventData) => void
  ): EventSource {
    const es = new EventSource(`/api/conversations/${convId}/stream`);

    es.addEventListener('init', (e) => {
      const data = JSON.parse((e as MessageEvent).data) as SseInitData;
      onEvent('init', data);
    });

    es.addEventListener('message', (e) => {
      const data = JSON.parse((e as MessageEvent).data) as SseMessageData;
      onEvent('message', data);
    });

    es.addEventListener('state_change', (e) => {
      const data = JSON.parse((e as MessageEvent).data) as SseStateChangeData;
      onEvent('state_change', data);
    });

    es.addEventListener('agent_done', () => {
      onEvent('agent_done', {});
    });

    es.addEventListener('error', () => {
      if (es.readyState === EventSource.CLOSED) {
        onEvent('disconnected', {});
      }
    });

    return es;
  },
};
