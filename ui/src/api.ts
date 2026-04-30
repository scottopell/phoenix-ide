// Phoenix API Client

// SSE event types come from the runtime schemas in `./sseSchemas`, which
// are typed against the Rust-generated wire shapes in `./generated/sse`
// via `v.GenericSchema<unknown, T>`. The `Sse*Data` names re-exported
// here are the schemas' *output* types (post-transform, so e.g.
// `conversation` is `Conversation`, not `unknown`). Rust-side drift
// surfaces at compile time when the generated type no longer satisfies
// the schema's target annotation. Task 02677.
export type {
  SseInitData,
  SseMessageData,
  SseMessageUpdatedData,
  SseStateChangeData,
  SseTokenData,
  SseConversationUpdateData,
  SseAgentDoneData,
  SseConversationBecameTerminalData,
  SseErrorData,
  SseBreadcrumb,
  ChainQaTokenData,
  ChainQaCompletedData,
  ChainQaFailedData,
} from './sseSchemas';

// Phoenix Chains v1 — generated wire shapes (Rust-derived via ts-rs).
// Re-exported here so chain-page components can import the chain page
// snapshot type and per-row Q&A history shape from a single import path.
import type { ChainView as ChainViewType } from './generated/ChainView';
import type { SubmitChainQaResponse as SubmitChainQaResponseType } from './generated/SubmitChainQaResponse';
import * as v from 'valibot';
import {
  ChainQaTokenSchema,
  ChainQaCompletedSchema,
  ChainQaFailedSchema,
  type ChainQaTokenData,
  type ChainQaCompletedData,
  type ChainQaFailedData,
} from './sseSchemas';
export type { ChainView } from './generated/ChainView';
export type { ChainMemberSummary } from './generated/ChainMemberSummary';
export type { ChainPosition } from './generated/ChainPosition';
export type { ChainQaRow } from './generated/ChainQaRow';
export type { ChainQaStatus } from './generated/ChainQaStatus';
export type { SubmitChainQaResponse } from './generated/SubmitChainQaResponse';

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
  /** Server-user's $SHELL (e.g. "/bin/zsh"); used to tailor the
   *  OSC 133 enablement snippet in the terminal HUD. REQ-TERM-017. */
  shell?: string | null;
  /** Server-user's $HOME (e.g. "/Users/alice"); used to scope seeded
   *  conversations for shell integration setup. REQ-SEED-*. */
  home_dir?: string | null;
  /** Seed parent conversation id (REQ-SEED-003). Decorative only. */
  seed_parent_id?: string | null;
  /** Seed label surfaced in the breadcrumb (REQ-SEED-004). */
  seed_label?: string | null;
  /** Slug of the seed parent, resolved server-side for the breadcrumb link.
   *  `null` if the parent has been deleted; UI renders unlinked text. */
  seed_parent_slug?: string | null;
  /** Continuation pointer (REQ-BED-030). If this conversation has been
   *  continued into a new conversation (context-exhausted handoff), this is
   *  the continuation's id. The UI uses this to (a) swap the Continue
   *  button for a "Continued in a new conversation" link on the parent, and
   *  (b) gate abandon / mark-as-merged on the parent (REQ-BED-031 — the
   *  action belongs on the continuation, enforced server-side with a 409
   *  `error_type = "continuation_exists"`). */
  continued_in_conv_id?: string | null;
  /** User-set name for the chain rooted at this conversation (REQ-CHN-007).
   *  Only meaningful on the root of a chain; non-root members will have
   *  this absent or null. The sidebar falls back to the root conversation's
   *  slug when this is null/absent. */
  chain_name?: string | null;
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
  | { type: 'awaiting_recovery'; message: string; recovery_kind: string }
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

export interface ModelInfo {
  id: string;
  provider: string;
  description: string;
  context_window: number;
  recommended: boolean;
}

export type GatewayStatus = 'not_configured' | 'healthy' | 'unreachable';

export type CredentialStatus = 'not_configured' | 'valid' | 'required' | 'running' | 'failed';

export interface ModelsResponse {
  models: ModelInfo[];
  default: string;
  /** Gateway reachability status determined at startup */
  gateway_status: GatewayStatus;
  /** True when at least one LLM provider is configured */
  llm_configured: boolean;
  credential_status: CredentialStatus;
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
  /** Absolute path to the task file on disk. */
  path: string;
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

/** 409 Conflict payload from the server. `conflict_slug` points at the
 *  conversation that owns the contested resource (e.g. an already-active
 *  Branch-mode conversation on the same branch). `continuation_id` is set
 *  when `error_type === 'continuation_exists'` (REQ-BED-031) so the UI can
 *  route to the continuation without parsing the error message. */
export interface ConflictErrorDetail {
  error: string;
  error_type: string;
  conflict_slug?: string;
  dirty_files?: string[];
  can_auto_stash?: boolean;
  continuation_id?: string;
}

/** Thrown by API methods that return 409 with a typed conflict payload. */
export class ConflictError extends Error {
  readonly detail: ConflictErrorDetail;

  constructor(detail: ConflictErrorDetail) {
    super(detail.error);
    this.name = 'ConflictError';
    this.detail = detail;
  }
}

export interface McpServerStatus {
  name: string;
  tool_count: number;
  tools: string[];
  enabled: boolean;
  pending_oauth_url?: string;
}

export interface McpReloadResult {
  added: string[];
  removed: string[];
  unchanged: string[];
}

export interface GitBranchEntry {
  name: string;
  local: boolean;
  remote: boolean;
  behind_remote?: number;
  /** Slug of an active conversation already using this branch (conflict). */
  conflict_slug?: string;
}

export interface GitBranchesResponse {
  branches: GitBranchEntry[];
  current: string;
  default_branch?: string;
}

export interface AuthStatus {
  auth_required: boolean;
  authenticated: boolean;
}

export interface UsageTotals {
  input_tokens: number;
  output_tokens: number;
  cache_creation_tokens: number;
  cache_read_tokens: number;
  turns: number;
}

export interface ConversationUsage {
  own: UsageTotals;
  total: UsageTotals;
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
    mode?: 'direct' | 'managed' | 'branch' | 'auto',
    baseBranch?: string | null,
    seedParentId?: string | null,
    seedLabel?: string | null,
  ): Promise<Conversation> {
    const body: Record<string, unknown> = { cwd, model, text, message_id: messageId, images, mode };
    if (baseBranch) {
      body['base_branch'] = baseBranch;
    }
    if (seedParentId) {
      body['seed_parent_id'] = seedParentId;
    }
    if (seedLabel) {
      body['seed_label'] = seedLabel;
    }
    const resp = await fetch('/api/conversations/new', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    });
    if (!resp.ok) {
      const err = await resp.json();
      if (resp.status === 409) {
        throw new ConflictError(err as ConflictErrorDetail);
      }
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

  async listGitBranches(cwd: string, search?: string): Promise<GitBranchesResponse> {
    let url = `/api/git/branches?cwd=${encodeURIComponent(cwd)}`;
    if (search) url += `&search=${encodeURIComponent(search)}`;
    const resp = await fetch(url);
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

  async getConversationUsage(convId: string): Promise<ConversationUsage> {
    const resp = await fetch(`/api/conversations/${convId}/usage`);
    if (!resp.ok) throw new Error('Failed to fetch usage');
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

  async abandonTask(convId: string): Promise<{ success: boolean }> {
    const resp = await fetch(`/api/conversations/${convId}/abandon-task`, { method: 'POST' });
    if (!resp.ok) { const err = await resp.json(); throw new Error(err.error || 'Failed to abandon task'); }
    return resp.json();
  },

  async markMerged(conversationId: string): Promise<{ success: boolean }> {
    const resp = await fetch(`/api/conversations/${conversationId}/mark-merged`, { method: 'POST' });
    if (!resp.ok) { const err = await resp.json(); throw new Error(err.error || 'Failed to mark as merged'); }
    return resp.json();
  },

  /** POST /api/conversations/:id/continue — context-exhausted handoff.
   *
   *  The endpoint is idempotent: if the parent already has a continuation,
   *  this returns that existing continuation with `already_existed: true`.
   *  Callers can therefore dispatch this unconditionally and let the server
   *  resolve the race (see REQ-BED-030 / task 24696 Phase 2).
   *
   *  Error shape:
   *   - 404 → `Error` (parent id not found)
   *   - 409 → `ConflictError` (parent not in context-exhausted state;
   *           `error_type = "parent_not_context_exhausted"`)
   *   - other non-2xx → generic `Error`
   */
  async continueConversation(convId: string): Promise<{
    conversation_id: string;
    slug?: string;
    already_existed: boolean;
  }> {
    const resp = await fetch(`/api/conversations/${convId}/continue`, { method: 'POST' });
    if (!resp.ok) {
      const err = await resp.json();
      if (resp.status === 409) {
        throw new ConflictError(err as ConflictErrorDetail);
      }
      if (resp.status === 404) {
        throw new Error(err.error || 'Conversation not found');
      }
      throw new Error(err.error || 'Failed to continue conversation');
    }
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

  // -----------------------------------------------------------------
  // Phoenix Chains v1 (REQ-CHN-003 / 004 / 005 / 007)
  //
  // The four endpoints below mirror `src/api/chains.rs`. Response
  // shapes come straight from the Rust-generated ts-rs types
  // (`ChainView`, `SubmitChainQaResponse`); SSE events are validated
  // by the chain Q&A schemas in `./sseSchemas`.
  // -----------------------------------------------------------------

  /** GET /api/chains/:rootId — full chain snapshot for the chain page. */
  async getChain(rootId: string): Promise<ChainViewType> {
    const resp = await fetch(`/api/chains/${encodeURIComponent(rootId)}`);
    if (resp.status === 404) throw new Error('Chain not found');
    if (!resp.ok) {
      const err = await resp.json().catch(() => ({}));
      throw new Error(err.error || 'Failed to load chain');
    }
    return resp.json();
  },

  /** POST /api/chains/:rootId/qa — submit a question. Returns synchronously
   *  with the `chain_qa_id`; tokens stream over the SSE endpoint and the
   *  persisted answer is fetched from `getChain` when complete. */
  async submitChainQuestion(
    rootId: string,
    question: string,
  ): Promise<SubmitChainQaResponseType> {
    const resp = await fetch(`/api/chains/${encodeURIComponent(rootId)}/qa`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ question }),
    });
    if (resp.status === 404) throw new Error('Chain not found');
    if (!resp.ok) {
      const err = await resp.json().catch(() => ({}));
      throw new Error(err.error || 'Failed to submit question');
    }
    return resp.json();
  },

  /** PATCH /api/chains/:rootId/name — set or clear the user-overridden
   *  chain name. Pass `null` to clear; the server falls back to the chain
   *  root's title for `display_name`. Returns the refreshed `ChainView`. */
  async setChainName(
    rootId: string,
    name: string | null,
  ): Promise<ChainViewType> {
    const resp = await fetch(`/api/chains/${encodeURIComponent(rootId)}/name`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name }),
    });
    if (resp.status === 404) throw new Error('Chain not found');
    if (!resp.ok) {
      const err = await resp.json().catch(() => ({}));
      throw new Error(err.error || 'Failed to set chain name');
    }
    return resp.json();
  },

};

// ---------------------------------------------------------------------------
// Chain SSE subscription
// ---------------------------------------------------------------------------

/** Discriminated union of chain Q&A events delivered over SSE. The `type`
 *  field matches the SSE `event:` label so consumers can dispatch on it
 *  without re-deriving the discriminator from the payload. */
export type ChainSseEventData =
  | ({ type: 'chain_qa_token' } & ChainQaTokenData)
  | ({ type: 'chain_qa_completed' } & ChainQaCompletedData)
  | ({ type: 'chain_qa_failed' } & ChainQaFailedData);

/** Subscribe to a chain's Q&A token stream. Returns an EventSource so the
 *  caller can `close()` it on unmount. Events that fail schema validation
 *  are reported via `onError` and dropped — the Rust-generated type is the
 *  source of truth, so a validation failure means a wire-format drift the
 *  server should be loud about (matches the conversation-SSE convention).
 *
 *  Multiple concurrent Q&As demux on `chain_qa_id`; the caller filters
 *  events to its own question id. */
export function subscribeToChainStream(
  rootId: string,
  onEvent: (event: ChainSseEventData) => void,
  onError?: (err: unknown) => void,
): EventSource {
  const source = new EventSource(`/api/chains/${encodeURIComponent(rootId)}/stream`);

  const handle = <T,>(
    eventName: 'chain_qa_token' | 'chain_qa_completed' | 'chain_qa_failed',
    schema: v.GenericSchema<unknown, T>,
  ) => {
    source.addEventListener(eventName, (msg) => {
      try {
        const raw: unknown = JSON.parse((msg as MessageEvent).data);
        const parsed = v.parse(schema, raw);
        onEvent({ type: eventName, ...parsed } as ChainSseEventData);
      } catch (err) {
        if (onError) onError(err);
      }
    });
  };

  handle('chain_qa_token', ChainQaTokenSchema);
  handle('chain_qa_completed', ChainQaCompletedSchema);
  handle('chain_qa_failed', ChainQaFailedSchema);

  if (onError) {
    source.addEventListener('error', (err) => onError(err));
  }
  return source;
}
