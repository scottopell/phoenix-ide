// Phoenix API Client

export interface Conversation {
  id: string;
  slug: string;
  model: string;
  cwd: string;
  created_at: string;
  updated_at: string;
  message_count: number;
  state?: ConversationState;
  archived?: boolean;
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
  context_window_size?: number;
}

export interface SseMessageData {
  message: Message;
}

export interface SseStateChangeData {
  state: ConversationState;
}

export type SseEventType = 'init' | 'message' | 'state_change' | 'agent_done' | 'disconnected';
export type SseEventData = SseInitData | SseMessageData | SseStateChangeData | Record<string, never>;

export interface ModelInfo {
  id: string;
  provider: string;
  description: string;
  context_window: number;
}

export interface ModelsResponse {
  models: ModelInfo[];
  default: string;
}

export const api = {
  async listConversations(): Promise<Conversation[]> {
    const resp = await fetch('/api/conversations');
    if (!resp.ok) throw new Error('Failed to list conversations');
    return (await resp.json()).conversations;
  },

  async createConversation(cwd: string, model?: string): Promise<Conversation> {
    const resp = await fetch('/api/conversations/new', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ cwd, model }),
    });
    if (!resp.ok) {
      const err = await resp.json();
      throw new Error(err.error || 'Failed to create conversation');
    }
    return (await resp.json()).conversation;
  },

  async getConversationBySlug(slug: string): Promise<{ conversation: Conversation; messages: Message[]; agent_working: boolean; context_window_size: number }> {
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

  async listDirectory(path: string): Promise<{ entries: { name: string; is_dir: boolean }[] }> {
    const resp = await fetch(`/api/list-directory?path=${encodeURIComponent(path)}`);
    if (!resp.ok) throw new Error('Failed to list directory');
    return resp.json();
  },

  async mkdir(path: string): Promise<{ created: boolean; error?: string }> {
    const resp = await fetch('/api/mkdir', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path }),
    });
    return resp.json();
  },

  async cancelConversation(convId: string): Promise<{ ok: boolean }> {
    const resp = await fetch(`/api/conversations/${convId}/cancel`, {
      method: 'POST',
    });
    if (!resp.ok) throw new Error('Failed to cancel');
    return resp.json();
  },

  async archiveConversation(convId: string): Promise<{ ok: boolean }> {
    const resp = await fetch(`/api/conversations/${convId}/archive`, {
      method: 'POST',
    });
    if (!resp.ok) throw new Error('Failed to archive');
    return resp.json();
  },

  async unarchiveConversation(convId: string): Promise<{ ok: boolean }> {
    const resp = await fetch(`/api/conversations/${convId}/unarchive`, {
      method: 'POST',
    });
    if (!resp.ok) throw new Error('Failed to unarchive');
    return resp.json();
  },

  async deleteConversation(convId: string): Promise<{ ok: boolean }> {
    const resp = await fetch(`/api/conversations/${convId}/delete`, {
      method: 'POST',
    });
    if (!resp.ok) throw new Error('Failed to delete');
    return resp.json();
  },

  async renameConversation(convId: string, name: string): Promise<{ ok: boolean }> {
    const resp = await fetch(`/api/conversations/${convId}/rename`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name }),
    });
    if (!resp.ok) {
      const err = await resp.json();
      throw new Error(err.error || 'Failed to rename');
    }
    return resp.json();
  },

  async listArchivedConversations(): Promise<Conversation[]> {
    const resp = await fetch('/api/conversations/archived');
    if (!resp.ok) throw new Error('Failed to list archived conversations');
    return (await resp.json()).conversations;
  },

  async listModels(): Promise<ModelsResponse> {
    const resp = await fetch('/api/models');
    if (!resp.ok) throw new Error('Failed to list models');
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
