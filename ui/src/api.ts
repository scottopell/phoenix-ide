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
  /** Semantic state category from API: idle, working, error, terminal */
  display_state?: 'idle' | 'working' | 'error' | 'terminal' | 'awaiting_approval';
  branch_name?: string | null;
  worktree_path?: string | null;
  base_branch?: string | null;
  commits_behind?: number;
  archived?: boolean;
  project_id?: string | null;
  conv_mode_label?: string;
}

export interface Project {
  id: string;
  canonical_path: string;
  main_ref: string;
  created_at: string;
  conversation_count: number;
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

export type ConversationState =
  | { type: 'idle' }
  | { type: 'awaiting_llm' }
  | { type: 'llm_requesting'; attempt: number }
  | { type: 'tool_executing'; current_tool: ToolCall; remaining_tools: ToolCall[] }
  | { type: 'awaiting_sub_agents'; pending: PendingSubAgent[]; completed_results: SubAgentResult[] }
  | { type: 'awaiting_continuation'; attempt: number }
  | { type: 'cancelling' }
  | { type: 'cancelling_tool'; current_tool: ToolCall }
  | { type: 'cancelling_sub_agents'; pending: PendingSubAgent[] }
  | { type: 'awaiting_task_approval'; title: string; priority: string; plan: string }
  | { type: 'context_exhausted'; summary: string }
  | { type: 'error'; message: string }
  | { type: 'terminal' };

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
  /** Semantic state category from API: idle, working, error, terminal */
  display_state?: string;
  last_sequence_id: number;
  /** Current context window usage in tokens */
  context_window_size?: number;
  /** Model's maximum context window in tokens */
  model_context_window?: number;
  breadcrumbs?: SseBreadcrumb[];
  /** How many commits the base branch is ahead of the task branch (Work mode only) */
  commits_behind?: number;
}

export interface SseMessageData {
  message: Message;
}

export interface SseStateChangeData {
  state: ConversationState;
  /** Semantic state category from API: idle, working, error, terminal */
  display_state?: string;
}

export type SseEventType = 'init' | 'message' | 'state_change' | 'agent_done' | 'conversation_update' | 'disconnected';
export type SseEventData = SseInitData | SseMessageData | SseStateChangeData | Record<string, never>;

export interface ModelInfo {
  id: string;
  provider: string;
  description: string;
  context_window: number;
  recommended: boolean;
}

export type GatewayStatus = 'not_configured' | 'healthy' | 'unreachable';

export interface ModelsResponse {
  models: ModelInfo[];
  default: string;
  /** Gateway reachability status determined at startup */
  gateway_status: GatewayStatus;
  /** True when at least one LLM provider is configured */
  llm_configured: boolean;
}

/** A single file search result from the conversation-scoped file search API (REQ-IR-004) */
export interface FileSearchEntry {
  path: string;
  is_text_file: boolean;
}

/** A single skill entry returned by the skills API (REQ-IR-005) */
export interface SkillEntry {
  name: string;
  description: string;
  argument_hint?: string | null;
}

/** Expansion error returned by the server when an @reference or /skill fails (REQ-IR-007) */
export interface ExpansionErrorDetail {
  error: string;
  error_type: 'file_not_found' | 'file_not_text' | 'skill_not_found';
  reference: string;
}

/**
 * Thrown by `api.sendMessage` when the server rejects a message due to an
 * unresolvable `@` reference (HTTP 422). Callers can `instanceof` check this
 * to distinguish expansion errors from network errors.
 */
export class ExpansionError extends Error {
  readonly detail: ExpansionErrorDetail;

  constructor(detail: ExpansionErrorDetail) {
    super(`expansion:${detail.error}`);
    this.name = 'ExpansionError';
    this.detail = detail;
  }
}

export const api = {
  async getProjects(): Promise<Project[]> {
    const resp = await fetch('/api/projects');
    if (!resp.ok) throw new Error('Failed to list projects');
    return resp.json();
  },

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

  async getConversationBySlug(slug: string): Promise<{ conversation: Conversation; messages: Message[]; agent_working: boolean; display_state: string; context_window_size: number }> {
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
    if (resp.status === 422) {
      // Expansion error — surface to InputArea as inline error (REQ-IR-007)
      const detail = await resp.json() as ExpansionErrorDetail;
      throw new ExpansionError(detail);
    }
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

  async listDirectory(path: string, signal?: AbortSignal): Promise<{ entries: { name: string; is_dir: boolean }[] }> {
    const resp = await fetch(`/api/list-directory?path=${encodeURIComponent(path)}`, signal ? { signal } : {});
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

  /** List skills available for a conversation's working directory (REQ-IR-005) */
  async listConversationSkills(
    convId: string,
    signal?: AbortSignal,
  ): Promise<{ skills: SkillEntry[] }> {
    const resp = await fetch(
      `/api/conversations/${convId}/skills`,
      signal ? { signal } : {},
    );
    if (!resp.ok) throw new Error('Failed to list skills');
    return resp.json();
  },

  /** Search files within a conversation's working directory (REQ-IR-004) */
  async searchConversationFiles(
    convId: string,
    query: string,
    limit = 50,
    signal?: AbortSignal,
  ): Promise<{ items: FileSearchEntry[] }> {
    const params = new URLSearchParams({ q: query, limit: String(limit) });
    const resp = await fetch(
      `/api/conversations/${convId}/files/search?${params}`,
      signal ? { signal } : {},
    );
    if (!resp.ok) throw new Error('Failed to search files');
    return resp.json();
  },

  async approveTask(convId: string): Promise<{ success: boolean; first_task?: boolean }> {
    const resp = await fetch(`/api/conversations/${convId}/approve-task`, { method: 'POST' });
    if (!resp.ok) { const err = await resp.json(); throw new Error(err.error || 'Failed to approve task'); }
    return resp.json();
  },

  async rejectTask(convId: string): Promise<{ success: boolean }> {
    const resp = await fetch(`/api/conversations/${convId}/reject-task`, { method: 'POST' });
    if (!resp.ok) { const err = await resp.json(); throw new Error(err.error || 'Failed to reject task'); }
    return resp.json();
  },

  async sendTaskFeedback(convId: string, annotations: string): Promise<{ success: boolean }> {
    const resp = await fetch(`/api/conversations/${convId}/task-feedback`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ annotations }),
    });
    if (!resp.ok) { const err = await resp.json(); throw new Error(err.error || 'Failed to send feedback'); }
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

    es.addEventListener('conversation_update', (e) => {
      const data = JSON.parse((e as MessageEvent).data);
      onEvent('conversation_update', data);
    });

    es.addEventListener('error', () => {
      if (es.readyState === EventSource.CLOSED) {
        onEvent('disconnected', {});
      }
    });

    return es;
  },
};
