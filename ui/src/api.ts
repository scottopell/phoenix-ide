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
  branch_name?: string | null;
  worktree_path?: string | null;
  base_branch?: string | null;
  task_title?: string | null;
  commits_behind?: number;
  commits_ahead?: number;
  archived?: boolean;
  project_id?: string | null;
  conv_mode_label?: string;
  project_name?: string | null;
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

export interface UserQuestion {
  question: string;
  header: string;
  options: QuestionOption[];
  multiSelect: boolean;
}

export interface QuestionOption {
  label: string;
  description?: string;
  preview?: string;
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
  | { type: 'awaiting_user_response'; questions: UserQuestion[] }
  | { type: 'context_exhausted'; summary: string }
  | { type: 'error'; message: string }
  | { type: 'terminal' };

/** Derive the coarse display category from a conversation's state type.
 *  Use this instead of reading `display_state` off the conversation object. */
export function getDisplayState(stateType: string | undefined): 'idle' | 'working' | 'error' | 'terminal' | 'awaiting_approval' {
  switch (stateType) {
    case 'idle': return 'idle';
    case 'terminal': return 'terminal';
    case 'error': return 'error';
    case 'context_exhausted': return 'terminal';
    case 'awaiting_task_approval': return 'awaiting_approval';
    case 'awaiting_user_response': return 'awaiting_approval';
    default: return stateType ? 'working' : 'idle';
  }
}

export interface ToolCall {
  id: string;
  input: { _tool?: string; [key: string]: unknown };
}

export interface Message {
  message_id: string;
  sequence_id: number;
  conversation_id: string;
  message_type: 'user' | 'agent' | 'tool' | 'system' | 'skill';
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
  commits_ahead?: number;
  project_name?: string | null;
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
  source: string;
  /** Absolute path to the SKILL.md file */
  path: string;
}

export interface TaskEntry {
  id: string;
  priority: string;
  status: string;
  slug: string;
  /** Slug of the conversation working on this task, if any. */
  conversation_slug?: string;
}

/** Expansion error returned by the server when an @reference or /skill fails (REQ-IR-007) */
export interface ExpansionErrorDetail {
  error: string;
  error_type: 'file_not_found' | 'file_not_text' | 'skill_invocation_failed';
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

export class DirtyMainError extends Error {
  readonly dirtyFiles: string[];
  readonly canAutoStash: boolean;

  constructor(message: string, dirtyFiles: string[], canAutoStash: boolean) {
    super(message);
    this.name = 'DirtyMainError';
    this.dirtyFiles = dirtyFiles;
    this.canAutoStash = canAutoStash;
  }
}

export interface McpServerStatus {
  name: string;
  tool_count: number;
  tools: string[];
  enabled: boolean;
}

export interface McpReloadResult {
  added: string[];
  removed: string[];
  unchanged: string[];
}

export interface GitBranchesResponse {
  branches: string[];
  current: string;
}

export interface AuthStatus {
  auth_required: boolean;
  authenticated: boolean;
}

export const api = {
  async authStatus(): Promise<AuthStatus> {
    const resp = await fetch('/api/auth/status');
    if (!resp.ok) throw new Error('Failed to check auth status');
    return resp.json();
  },

  async login(password: string): Promise<void> {
    const resp = await fetch('/api/auth/login', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ password }),
    });
    if (!resp.ok) {
      const err = await resp.json() as { error?: string };
      throw new Error(err.error ?? 'Login failed');
    }
  },

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
    images: ImageData[] = [],
    mode?: 'direct' | 'managed',
    baseBranch?: string | null,
  ): Promise<Conversation> {
    const body: Record<string, unknown> = { cwd, model, text, message_id: messageId, images, mode };
    if (baseBranch) {
      body['base_branch'] = baseBranch;
    }
    const resp = await fetch('/api/conversations/new', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
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

  async validateCwd(path: string): Promise<{ valid: boolean; error?: string; is_git: boolean }> {
    const resp = await fetch(`/api/validate-cwd?path=${encodeURIComponent(path)}`);
    return resp.json();
  },

  async listGitBranches(cwd: string): Promise<GitBranchesResponse> {
    const resp = await fetch(`/api/git/branches?cwd=${encodeURIComponent(cwd)}`);
    if (!resp.ok) throw new Error('Failed to list git branches');
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

  /** List tasks from the conversation's project tasks/ directory */
  async listConversationTasks(
    convId: string,
    signal?: AbortSignal,
  ): Promise<{ tasks: TaskEntry[] }> {
    const resp = await fetch(
      `/api/conversations/${convId}/tasks`,
      signal ? { signal } : {},
    );
    if (!resp.ok) throw new Error('Failed to list tasks');
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

  async completeTask(convId: string, autoStash = false): Promise<{ success: boolean; commit_message: string; task_not_done?: boolean }> {
    const qs = autoStash ? '?auto_stash=true' : '';
    const resp = await fetch(`/api/conversations/${convId}/complete-task${qs}`, { method: 'POST' });
    if (!resp.ok) {
      const err = await resp.json();
      if (err.error_type === 'dirty_main_checkout') {
        throw new DirtyMainError(err.error, err.dirty_files || [], err.can_auto_stash || false);
      }
      throw new Error(err.error || 'Failed to start completion');
    }
    return resp.json();
  },

  async confirmComplete(convId: string, commitMessage: string, autoStash = false): Promise<{ success: boolean; commit_sha: string }> {
    const resp = await fetch(`/api/conversations/${convId}/confirm-complete`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ commit_message: commitMessage, auto_stash: autoStash }),
    });
    if (!resp.ok) { const err = await resp.json(); throw new Error(err.error || 'Failed to confirm completion'); }
    return resp.json();
  },

  async abandonTask(convId: string): Promise<{ success: boolean }> {
    const resp = await fetch(`/api/conversations/${convId}/abandon-task`, { method: 'POST' });
    if (!resp.ok) { const err = await resp.json(); throw new Error(err.error || 'Failed to abandon task'); }
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

  async respondToQuestion(
    convId: string,
    answers: Record<string, string>,
    annotations?: Record<string, { notes?: string; preview?: string }>,
  ): Promise<{ success: boolean }> {
    const resp = await fetch(`/api/conversations/${convId}/respond`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ answers, annotations }),
    });
    if (!resp.ok) { const err = await resp.json(); throw new Error(err.error || 'Failed to respond to question'); }
    return resp.json();
  },

  async getMcpStatus(): Promise<McpServerStatus[]> {
    const resp = await fetch('/api/mcp/status');
    if (!resp.ok) throw new Error('Failed to get MCP status');
    return resp.json();
  },

  async upgradeModel(conversationId: string, model: string): Promise<void> {
    const resp = await fetch(`/api/conversations/${conversationId}/upgrade-model`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ model }),
    });
    if (!resp.ok) {
      const err = await resp.json();
      throw new Error(err.error || 'Failed to upgrade model');
    }
  },

  async reloadMcp(): Promise<McpReloadResult> {
    const resp = await fetch('/api/mcp/reload', { method: 'POST' });
    if (!resp.ok) throw new Error('Failed to reload MCP servers');
    return resp.json();
  },

  async disableMcpServer(name: string): Promise<void> {
    const resp = await fetch(`/api/mcp/servers/${encodeURIComponent(name)}/disable`, { method: 'POST' });
    if (!resp.ok) throw new Error('Failed to disable MCP server');
  },

  async enableMcpServer(name: string): Promise<void> {
    const resp = await fetch(`/api/mcp/servers/${encodeURIComponent(name)}/enable`, { method: 'POST' });
    if (!resp.ok) throw new Error('Failed to enable MCP server');
  },

  /** Fetch conversation data via share token (REQ-AUTH-006) */
  async getSharedConversation(token: string): Promise<{
    conversation: Conversation;
    messages: Message[];
    agent_working: boolean;
    display_state: string;
    context_window_size: number;
  }> {
    const resp = await fetch(`/api/share/${encodeURIComponent(token)}/conversation`);
    if (resp.status === 404) throw new Error('Share link not found or has been revoked');
    if (!resp.ok) throw new Error('Failed to load shared conversation');
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
