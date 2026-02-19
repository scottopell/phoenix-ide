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

export interface PendingSubAgent {
  agent_id: string;
  task: string;
}

export interface SubAgentOutcome {
  type: 'success' | 'failure';
  result?: string;
  error?: string;
  error_kind?: string;
}

export interface SubAgentResult {
  agent_id: string;
  task: string;
  outcome: SubAgentOutcome;
}

export interface ConversationState {
  type: string;
  attempt?: number;
  current_tool?: ToolCall;
  remaining_tools?: ToolCall[];
  // Sub-agent state
  pending?: PendingSubAgent[];
  completed_results?: SubAgentResult[];
  message?: string;
}

export interface ToolCall {
  id: string;
  input: { _tool?: string; [key: string]: unknown };
}

export interface Message {
  message_id: string;
  sequence_id: number;
  conversation_id: string;
  message_type: 'user' | 'agent' | 'tool';
  type?: string; // legacy
  content: MessageContent;
  display_data?: ImageData | Record<string, unknown>; // For tool results with images (e.g., screenshots)
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
  /** For bash tool_use blocks, the cleaned display command (cd prefix stripped) */
  display?: string;
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

export interface SseBreadcrumb {
  type: 'user' | 'llm' | 'tool' | 'subagents';
  label: string;
  tool_id?: string;
  sequence_id?: number;
  preview?: string;
}

export interface SseInitData {
  conversation: Conversation;
  messages: Message[];
  agent_working: boolean;
  last_sequence_id: number;
  /** Current context window usage in tokens */
  context_window_size?: number;
  /** Model's maximum context window in tokens */
  model_context_window?: number;
  breadcrumbs?: SseBreadcrumb[];
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

  async createConversation(
    cwd: string,
    text: string,
    messageId: string,
    model?: string,
    images: ImageData[] = []
  ): Promise<Conversation> {
    const resp = await fetch('/api/conversations/new', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ cwd, model, text, message_id: messageId, images }),
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

  async sendMessage(
    convId: string,
    text: string,
    images: ImageData[] = [],
    localId: string,
  ): Promise<{ queued: boolean }> {
    const resp = await fetch(`/api/conversations/${convId}/chat`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        text,
        images,
        message_id: localId,
        user_agent: navigator.userAgent,
      }),
    });
    if (!resp.ok) throw new Error('Failed to send message');
    return resp.json();
  },

  async getSystemPrompt(convId: string): Promise<string> {
    const resp = await fetch(`/api/conversations/${convId}/system-prompt`);
    if (!resp.ok) throw new Error('Failed to fetch system prompt');
    return (await resp.json()).system_prompt;
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

  /** Manually trigger context continuation (REQ-BED-023) */
  async triggerContinuation(convId: string): Promise<{ success: boolean }> {
    const resp = await fetch(`/api/conversations/${convId}/trigger-continuation`, {
      method: 'POST',
    });
    if (!resp.ok) throw new Error('Failed to trigger continuation');
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

  async getEnv(): Promise<{ home_dir: string }> {
    const resp = await fetch('/api/env');
    if (!resp.ok) throw new Error('Failed to get environment info');
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
