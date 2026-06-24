import type { ErrorPresentation } from './errorPresentation';
import type { ErrorKind } from './generated/ErrorKind';
import type { DeploymentInfo } from './generated/DeploymentInfo';
import type { FileViewerKind } from './generated/FileViewerKind';
import type { UsageOverview } from './generated/UsageOverview';
import type { ConversationUsageDetail } from './generated/ConversationUsageDetail';
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
export type { ChainWorkIdentity } from './generated/ChainWorkIdentity';
export type { ChainMemberSummary } from './generated/ChainMemberSummary';
export type { ChainPosition } from './generated/ChainPosition';
export type { ChainQaRow } from './generated/ChainQaRow';
export type { ChainQaStatus } from './generated/ChainQaStatus';
export type { SubmitChainQaResponse } from './generated/SubmitChainQaResponse';
export type {
  WorkScopeInventory,
  BashHandleInventory,
  BashHandleState,
  TmuxInventory,
  TmuxServerStatus,
  BrowserInventory,
  BrowserSessionLiveness,
} from './generated/sse';
import type { WorkScopeInventory as WorkScopeInventoryType } from './generated/sse';
export type { BashHandleInspection } from './generated/BashHandleInspection';
export type { ResourceSample } from './generated/ResourceSample';
export type { BashRingWindow } from './generated/BashRingWindow';
export type { BashRingLine } from './generated/BashRingLine';
import type { BashHandleInspection as BashHandleInspectionType } from './generated/BashHandleInspection';

export interface Conversation {
  id: string;
  slug: string;
  model: string;
  cwd: string;
  created_at: string;
  updated_at: string;
  message_count: number;
  state?: ConversationState;
  /** RFC3339 server clock at which the conversation entered its current
   *  `state`. Bumped on every transition by the runtime (db.rs:676);
   *  re-emitted on every `StateChange` SSE event for parity with this
   *  Init carrier. The SSE Init handler converts to ms via
   *  `Date.parse(s)` once before storing on the atom as
   *  `phaseStateUpdatedAt`. Specs:
   *  `specs/working-phase-visibility/` REQ-WPV-001. */
  state_updated_at?: string;
  branch_name?: string | null;
  worktree_path?: string | null;
  base_branch?: string | null;
  task_title?: string | null;
  archived?: boolean;
  project_id?: string | null;
  conv_mode_label?: string;
  project_name?: string | null;
  parent_conversation_id?: string | null;
  /** Slug of the sub-agent's parent conversation, resolved server-side for the
   *  breadcrumb link (mirrors `seed_parent_slug`). `null`/absent when this is
   *  not a sub-agent or the parent has been deleted; the UI renders unlinked
   *  text in the latter case. */
  parent_conversation_slug?: string | null;
  user_initiated?: boolean;
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
  /** Server-computed presentation mode: idle | working | needs_action | error | done.
   *  Single source of truth for which visual indicator to render. */
  presentation_mode?: string;
  /** Whether the conversation currently has a live browser session in the
   *  server's `BrowserSessionManager`. Authoritative for UI gating of the
   *  live-browser-view affordances (REQ-BT-018). Hydrated at SSE init from
   *  the manager's `HashMap` and updated by `browser_session_state` events
   *  on this conversation's stream. Always populated by the server. */
  browser_session_active: boolean;
  /** True when this conversation's in-app terminal is backed by a
   *  per-conversation tmux session (server has tmux on PATH). The composer
   *  uses this to label terminal-selection snippets with the tmux pane id,
   *  so the LLM can follow up via the existing `tmux` tool to capture more
   *  of the pane on demand. Server populates from
   *  `TmuxRegistry::binary_available()` on every payload — SSE init *and*
   *  list-endpoint serialization route through `enrich_conversation_with_runtime`,
   *  so the 5s poll's snapshot upsert never clobbers a true value back to
   *  false (regression history: PR #92). */
  terminal_uses_tmux: boolean;
  /** `WorkScope::stable_key()` for this conversation's resolved work scope
   *  (e.g. `worktree:/path`, `conversation:<id>`, `global:`). Used to build
   *  the work-scope inventory URL `GET /api/work-scope/:scope_key/inventory`.
   *  Server-resolved from the conversation id + worktree path; always
   *  populated. */
  work_scope_key: string;
  cached_pr?: CachedPrSummary | null;
}

export type PrUnavailableReason = 'gh_missing' | 'not_authenticated' | 'not_git_repo' | 'command_failed';
export type PrCheckState = 'passing' | 'pending' | 'failing' | 'unknown';
export type PrDisplayState = 'open' | 'draft' | 'merged' | 'closed';

export interface CachedPrSummary {
  number: number;
  title: string;
  url: string;
  display_state: PrDisplayState;
  base: string;
  head: string;
}

export interface PrCheckSummary {
  passing: number;
  pending: number;
  failing: number;
  skipped: number;
  unknown: number;
  failing_names: string[];
  pending_names: string[];
}

export type PrFeedbackSource = 'issue_comment' | 'review_comment' | 'review_summary' | 'review_thread';
export type PrFeedbackCoverageSurface = 'issue_comments' | 'review_comments' | 'review_summaries' | 'review_threads';
export type PrFeedbackCoverageStatus = 'fetched' | 'unavailable' | 'auth_failed';

export interface PrFeedbackCoverage {
  surface: PrFeedbackCoverageSurface;
  status: PrFeedbackCoverageStatus;
  detail?: string;
}

export interface PrFeedbackItem {
  id?: string;
  /** GraphQL `pullRequestReviewThread` node id (`PRRT_…`); present only for review_thread items. */
  thread_id?: string;
  source: PrFeedbackSource;
  author: string;
  body: string;
  path?: string;
  url?: string;
  created_at?: string;
  resolved?: boolean;
}

export interface PrFeedbackSummary {
  total: number;
  unresolved: number;
  items: PrFeedbackItem[];
  coverage: PrFeedbackCoverage[];
}

export interface PrAutoFixContextResponse {
  artifact_path: string;
  pr_number: number;
  message: string;
}

export type PrRefreshState = 'fresh' | 'unavailable' | 'not_found';

export interface PrIdentity {
  number: number;
  title: string;
  url: string;
  state: string;
  draft: boolean;
  display_state: PrDisplayState;
  base: string;
  head: string;
  updated_at?: string;
}

export interface PrRefreshMetadata {
  state: PrRefreshState;
  reason?: PrUnavailableReason;
  last_attempted_at: string;
  last_refreshed_at?: string;
  stale: boolean;
}

// Mirrors the Rust `PrFeedbackFreshness` (api/types.rs): an internally-tagged
// union where each variant carries its own count. Coverage gaps / fetch errors
// are not represented here — they are a separate error condition
// (`PrFeedbackCoverageHealth`).
export type PrFeedbackFreshness =
  | { state: 'new'; count: number }
  | { state: 'edited'; count: number };

// Mirrors the Rust `PrFeedbackCoverageHealth` (api/types.rs). Orthogonal to
// freshness: when a surface can't be read, any freshness count is a lower
// bound. `auth_required` is user-actionable (gh auth login); `incomplete` is
// transient.
export type PrFeedbackCoverageHealth =
  | { kind: 'auth_required'; surfaces: PrFeedbackCoverageSurface[] }
  | { kind: 'incomplete'; surfaces: PrFeedbackCoverageSurface[] };

export type WorkChangeNeedsReviewReason =
  | 'uncommitted_changes'
  | 'branch_not_pushed'
  | 'local_ahead_of_remote'
  | 'remote_diverged'
  | 'non_github_remote'
  | 'unknown_remote'
  | 'unknown';

export type WorkChangeSummary =
  | { kind: 'clean' }
  | {
      kind: 'dirty_pr_ready';
      create_pr_url: string;
      branch_name: string;
      base_branch: string;
    }
  | { kind: 'dirty_needs_review'; reason: WorkChangeNeedsReviewReason }
  | { kind: 'loading' }
  | { kind: 'unavailable'; reason: string };

export interface PrStatusResponse {
  found: boolean;
  pr?: PrIdentity;
  refresh: PrRefreshMetadata;
  unavailable_reason?: PrUnavailableReason;
  number?: number;
  title?: string;
  url?: string;
  state?: string;
  draft?: boolean;
  base?: string;
  head?: string;
  check_state?: PrCheckState;
  check_summary?: PrCheckSummary;
  feedback_summary?: PrFeedbackSummary;
  updated_at?: string;
  display_state?: PrDisplayState;
  feedback_freshness?: PrFeedbackFreshness;
  feedback_coverage?: PrFeedbackCoverageHealth;
  work_change: WorkChangeSummary;
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

export type SubAgentOutcome =
  | { type: 'success'; result?: string }
  | { type: 'failure'; error?: string; error_kind?: string }
  | { type: 'timed_out' };

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

export interface ContinuationSummaryRequest {
  rejected_tool_calls: ToolCall[];
}

export type RecoveryResumeTarget =
  | { type: 'conversation_turn' }
  | { type: 'continuation_summary'; request: ContinuationSummaryRequest };

export interface CommissionReviewApprovalScope {
  kind: string;
  repo_root: string;
  base: string;
  head: string;
  dirty: boolean;
  changed_files: number;
  insertions: number;
  deletions: number;
}

export type ConversationState =
  | { type: 'idle' }
  | { type: 'awaiting_llm' }
  | { type: 'llm_requesting'; attempt: number }
  | { type: 'seeded_llm_requesting'; seed_message_id: string; attempt: number }
  | { type: 'tool_executing'; current_tool: ToolCall; remaining_tools: ToolCall[] }
  | { type: 'awaiting_sub_agents'; pending: PendingSubAgent[]; completed_results: SubAgentResult[] }
  | { type: 'awaiting_continuation'; attempt: number }
  | { type: 'cancelling' }
  | { type: 'cancelling_tool'; current_tool: ToolCall }
  | { type: 'cancelling_sub_agents'; pending: PendingSubAgent[] }
  | { type: 'awaiting_task_approval'; title: string; priority: string; plan: string }
  | {
      type: 'awaiting_commission_review_approval';
      brief: string;
      focus?: string | null;
      allow_dirty_working_tree: boolean;
      scope: CommissionReviewApprovalScope | undefined;
    }
  | { type: 'awaiting_user_response'; questions: UserQuestion[] }
  | { type: 'context_exhausted'; summary: string }
  | { type: 'handed_off'; successor_conv_id: string }
  | { type: 'error'; message: string; error_kind: ErrorKind; error?: ErrorPresentation }
  | { type: 'awaiting_recovery'; message: string; recovery_kind: string; resume: RecoveryResumeTarget }
  | { type: 'terminal' };

/** Mirror of the backend `ConvState::allows_model_change`: true only for
 *  `idle` and `error`. */
export function canChangeModelInState(state: ConversationState): boolean {
  return state.type === 'idle' || state.type === 'error';
}

/** A conversation in a terminal state can no longer act on its pending fork
 *  proposals — the backend retires them to `dismissed` (merged / abandoned →
 *  `terminal`, context-exhausted → `context_exhausted`, handed off →
 *  `handed_off`). The exhaustive switch makes a new backend state a compile
 *  error here rather than a silently-missed retirement signal. */
export function isTerminalConversationState(state: ConversationState): boolean {
  switch (state.type) {
    case 'terminal':
    case 'context_exhausted':
    case 'handed_off':
      return true;
    case 'idle':
    case 'awaiting_llm':
    case 'llm_requesting':
    case 'seeded_llm_requesting':
    case 'tool_executing':
    case 'awaiting_sub_agents':
    case 'awaiting_continuation':
    case 'cancelling':
    case 'cancelling_tool':
    case 'cancelling_sub_agents':
    case 'awaiting_task_approval':
    case 'awaiting_commission_review_approval':
    case 'awaiting_user_response':
    case 'error':
    case 'awaiting_recovery':
      return false;
    default:
      state satisfies never;
      return false;
  }
}

/** Derive the coarse display category from a conversation's raw state type string.
 *  Internal fallback — prefer `getConvDisplayState` which reads `presentation_mode`. */
function getDisplayState(stateType: string | undefined): 'idle' | 'working' | 'error' | 'terminal' | 'awaiting-approval' {
  switch (stateType) {
    case 'idle': return 'idle';
    case 'terminal': return 'terminal';
    case 'handed_off': return 'terminal';
    case 'error': return 'error';
    case 'context_exhausted': return 'idle';
    case 'awaiting_task_approval': return 'awaiting-approval';
    case 'awaiting_commission_review_approval': return 'awaiting-approval';
    case 'awaiting_user_response': return 'awaiting-approval';
    default: return stateType ? 'working' : 'idle';
  }
}

/** Derive the coarse display category for a conversation.
 *  Reads `presentation_mode` from the server when available; falls back to
 *  `state.type` for SSE-only views that may not have received a REST snapshot. */
export function getConvDisplayState(conv: Conversation | undefined): 'idle' | 'working' | 'error' | 'terminal' | 'awaiting-approval' {
  switch (conv?.presentation_mode) {
    case 'idle': return 'idle';
    case 'working': return 'working';
    case 'needs_action': return 'awaiting-approval';
    case 'error': return 'error';
    case 'done': return 'terminal';
    default: return getDisplayState(conv?.state?.type);
  }
}


export interface ToolCall {
  id: string;
  /** Authoritative tool name. Emitted by ToolCall's custom Serialize impl on
   *  the Rust side; also supplied by the NotifyClient state_change summary. */
  name: string;
  input: { _tool?: string; [key: string]: unknown };
}

export interface Message {
  message_id: string;
  sequence_id: number;
  conversation_id: string;
  message_type: 'user' | 'agent' | 'tool' | 'system' | 'error' | 'continuation' | 'skill';
  type?: string; // legacy
  content: MessageContent;
  display_data?: ImageData | Record<string, unknown> | null; // For tool results with images (e.g., screenshots)
  usage_data?: UsageData;
  created_at: string;
}

export type MessageContent = 
  | { text: string; images?: ImageData[]; files?: FileAttachment[] }  // user message
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
  /** Typed image payloads (mirrors Rust `ToolContent.images`). Single source
   *  of truth for tool-result images — not duplicated into `display_data`. */
  images?: ImageData[];
}

export interface ImageData {
  data: string;
  media_type: string;
}

export interface FileAttachment {
  original_name: string;
  media_type: string;
  size_bytes: number;
  stored_path: string;
}

export const MAX_FILE_ATTACHMENT_SIZE = 10 * 1024 * 1024;
export const MAX_TOTAL_FILE_ATTACHMENT_SIZE = 25 * 1024 * 1024;
export const MAX_FILE_ATTACHMENTS = 10;

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

export type CredentialStatus = 'not_configured' | 'valid' | 'required' | 'running' | 'failed';

export interface ModelsResponse {
  models: ModelInfo[];
  default: string;
  /** True when at least one LLM provider is configured */
  llm_configured: boolean;
  credential_status: CredentialStatus;
}

/** A single file search result from the conversation-scoped file search API (REQ-IR-004) */
export interface FileSearchEntry {
  path: string;
  /** Server's verdict on how the viewer treats this path — shared with the
   *  sidebar and quick-open via the same classifier. */
  viewer: FileViewerKind;
}

export interface CodeSearchEntry {
  path: string;
  line_number: number;
  line_text: string;
  match_start: number;
  match_end: number;
}

/** A single skill entry returned by the skills API (REQ-IR-005, REQ-BS-003) */
export interface SkillEntry {
  name: string;
  description: string;
  argument_hint?: string | null;
  /**
   * Where this skill was discovered. Either a discovery directory like
   * `".claude/skills"` / `".agents/skills"` for filesystem skills, or the
   * literal `"builtin"` for skills bundled with the phoenix binary
   * (extracted to `<HOME>/.phoenix-ide/builtin-skills/` at startup).
   */
  source: string;
  /** Absolute path to the SKILL.md file. Always populated; built-in skills
   *  point at their extracted location. */
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

/** Lightweight task status counts for the Tasks panel's collapsed header. */
export interface TaskCountResponse {
  active: number;
  closed: number;
  blocked: number;
  /** Whether the branch-derived current task is present among the tasks. */
  current: boolean;
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

/** Thrown by API methods when the addressed resource is gone (404). A typed
 *  error so callers can distinguish a definitive "no longer exists" outcome
 *  from a transient/ambiguous failure rather than string-matching a message. */
export class NotFoundError extends Error {
  constructor(message = 'Not found') {
    super(message);
    this.name = 'NotFoundError';
  }
}

/** Lifecycle status of a decoupled task fork proposal (REQ-PROJ-034 / 037).
 *  `pending` is the only reviewable state; the other three are terminal
 *  resolutions and withdraw the Review affordance. Mirrors the Rust
 *  `ForkProposalStatus` serialization. */
export type ForkProposalStatus = 'pending' | 'spawned' | 'dismissed' | 'promoted';

/** One fork proposal as rendered to the review surface (REQ-PROJ-034).
 *  Hand-written to mirror the plain-`Serialize` Rust `ForkProposalSummary`
 *  — that module carries no `ts_rs::TS` derives, so there is no generated
 *  type to import (matches the rest of this client). */
export interface ForkProposalSummary {
  id: string;
  status: ForkProposalStatus;
  title: string;
  priority: string;
  task_file: string;
  /** Snapshotted brief body. The snapshot is not in the transcript, so the
   *  review modal renders the brief from here, keyed by proposal id. */
  body: string;
  /** Set once a proposal is `spawned` — the Work fork's conversation id. */
  fork_conversation_id?: string;
  /** Set once a proposal is `promoted` — the Explore refinement's id. */
  refinement_conversation_id?: string;
}

export type McpConnState = 'ready' | 'unauthorized' | 'failed';
export type McpTransportKind = 'stdio' | 'http';
export type McpAuthKind = 'none' | 'static' | 'oauth';

export interface McpServerStatus {
  name: string;
  /** Lifecycle state surfaced for the panel (REQ-MCP-013, REQ-MCP-018). */
  state: McpConnState;
  transport: McpTransportKind;
  auth: McpAuthKind;
  tool_count: number;
  tools: string[];
  enabled: boolean;
  /** Set while awaiting the user to complete an OAuth flow (state = unauthorized). */
  pending_oauth_url?: string;
  /** Failure cause when state = failed; cleared on a successful reconnect. */
  last_error?: string;
  /** On an unauthorized entry, a diagnostic when the OAuth redirect base is
   *  unreachable from another machine, so the authorize link will fail. */
  auth_redirect_warning?: string;
}

export interface McpReloadFailure {
  server: string;
  action: string;
  error: string;
}

export interface McpReloadResult {
  added: string[];
  removed: string[];
  restarted: string[];
  unchanged: string[];
  failed: McpReloadFailure[];
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

export interface NotificationSettings {
  enabled: boolean;
  notify_task_approval: boolean;
  notify_question: boolean;
  notify_error: boolean;
  notify_idle: boolean;
}

/** Global default LLM language for new conversations. The selected
 * language is pinned to each conversation at creation time; chain
 * continuations and sub-agents inherit from their parent rather than
 * re-reading this default. */
export interface LlmLanguagePrompts {
  base_prompt: string;
  explore_mode_block_template: string;
  work_mode_block_template: string;
  direct_mode_block: string;
  branch_mode_block_template: string;
  sub_agent_suffix: string;
  next_task_hint_template: string;
  pr_autofix_instruction_template: string;
}

export interface LlmLanguageCatalogEntry {
  id: string;
  label: string;
  description: string;
  prompts: LlmLanguagePrompts;
}

export interface LlmLanguageSetting {
  language: string;
  available: string[];
  languages: LlmLanguageCatalogEntry[];
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

// ---- Codex / ChatGPT OAuth login (task 27104) ----

export interface CodexLoginPreflight {
  auth_path: string;
  piggyback_path: string;
  already_signed_in: boolean;
  bridge_loaded_at_startup: boolean;
  /// True when an in-app login won't take effect until Phoenix restarts —
  /// either because no credential was loaded at startup or because the
  /// loaded credential is pinned to a different file (the piggyback case).
  restart_required_after_login: boolean;
  piggyback_env_set: boolean;
  account_id: string | null;
  account_email: string | null;
}

export interface CodexManualCodeRequest {
  /// Either the full post-callback URL (preferred — backend extracts code+state),
  /// or both `code` and `state` if the UI parsed them itself.
  redirect_url?: string;
  code?: string;
  state?: string;
}

export interface CodexPkceStartResponse {
  session_id: string;
  authorize_url: string;
  redirect_uri: string;
  loopback_bound: boolean;
  callback_port: number;
}

export interface CodexDeviceStartResponse {
  session_id: string;
  verification_url: string;
  user_code: string;
  interval_secs: number;
  timeout_secs: number;
}

export type CodexLoginStatus =
  | { kind: 'pending' }
  | { kind: 'success'; account_id: string | null; auth_path: string }
  | { kind: 'error'; message: string };

/**
 * Selects the root that directory-scoped discovery (`@file` / `/skill`)
 * resolves against, for the new-conversation composer. `mode`/`baseBranch`
 * mirror the create-time submission: a branch/managed workflow discovers
 * against the chosen branch's committed tree (what its worktree will hold),
 * so suggestions match what create-time expansion can resolve. Omitted ⇒
 * Direct (the working directory).
 */
export interface ProjectResolutionOpts {
  // Explicit `| undefined` so a caller can pass `{ mode: undefined }` under
  // exactOptionalPropertyTypes (the in-conversation composer leaves these unset).
  mode?: 'direct' | 'managed' | 'branch' | undefined;
  baseBranch?: string | null | undefined;
}

function applyResolutionOpts(params: URLSearchParams, opts?: ProjectResolutionOpts): void {
  if (opts?.mode) params.set('mode', opts.mode);
  if (opts?.baseBranch) params.set('base_branch', opts.baseBranch);
}

export type ServiceStatus = 'healthy' | 'stale';
export type DiscoveryConfidence = 'explicit_api_catalog';
export type DiscoverySource = 'loopback_probe';

export type ServiceCapability =
  | { kind: 'api_catalog'; url: string }
  | { kind: 'open_api'; url: string; title: string | null; content_type: string | null }
  | { kind: 'documentation'; url: string; title: string | null }
  | { kind: 'html_ui'; url: string; title: string | null }
  | { kind: 'other_link'; rel: string; url: string; title: string | null; content_type: string | null };

export interface DiscoveredService {
  id: string;
  base_url: string;
  host: string;
  port: number;
  title: string | null;
  description: string | null;
  capabilities: ServiceCapability[];
  first_seen_at: string;
  last_seen_at: string;
  status: ServiceStatus;
  confidence: DiscoveryConfidence;
  source: DiscoverySource;
}


export type AnalyticsFidelityValue = 'native' | 'derived' | 'estimated' | 'unknown' | 'unavailable';

export interface AnalyticsFidelity {
  tokens: AnalyticsFidelityValue;
  cost: AnalyticsFidelityValue;
  tool_calls: AnalyticsFidelityValue;
  first_byte: AnalyticsFidelityValue;
  retries: AnalyticsFidelityValue;
  outcomes: AnalyticsFidelityValue;
  lifecycle: AnalyticsFidelityValue;
}

export interface AnalyticsTokenTotals {
  input_tokens: number;
  output_tokens: number;
  cache_creation_tokens: number;
  cache_read_tokens: number;
}

export interface AnalyticsUsageTurn {
  turn_usage_id: number;
  conversation_id: string;
  root_conversation_id: string;
  model: string;
  created_at: string;
  first_byte_at: string | null;
  first_byte_latency_ms: number | null;
  tokens: AnalyticsTokenTotals;
  cost: {
    input_usd: number | null;
    output_usd: number | null;
    cache_write_usd: number | null;
    cache_read_usd: number | null;
    total_usd: number | null;
    pricing_known: boolean;
  };
}

export interface AnalyticsToolCall {
  conversation_id: string;
  assistant_message_id: string;
  tool_result_message_id: string | null;
  tool_use_id: string;
  tool_name: string;
  is_error: boolean;
  denied: boolean;
  duration_ms: number | null;
  normalized_command: string | null;
  touched_files: string[];
}

export interface AnalyticsSession {
  session_id: string;
  root_session_id: string;
  project_id: string | null;
  cwd: string;
  worktree_path: string | null;
  task_id: string | null;
  task_title: string | null;
  branch: string | null;
  started_at: string;
  last_seen_at: string;
  ended_at: string | null;
  terminal_status: string | null;
  turns: AnalyticsUsageTurn[];
  tool_calls: AnalyticsToolCall[];
  fidelity: AnalyticsFidelity;
}

export interface TrajectoryExportPayload {
  client: 'phoenix';
  source: string;
  session: AnalyticsSession;
}

export interface DiscoveryServicesResponse {
  services: DiscoveredService[];
}

export const api = {
  async authStatus(): Promise<AuthStatus> {
    const resp = await fetch('/api/auth/status');
    if (!resp.ok) throw new Error('Failed to check auth status');
    return resp.json();
  },

  async getVersion(): Promise<{ version: string; git_sha: string }> {
    const resp = await fetch('/api/version');
    if (!resp.ok) throw new Error('Failed to load version');
    return resp.json();
  },

  async deploymentInfo(): Promise<DeploymentInfo> {
    const resp = await fetch('/api/deployment');
    if (!resp.ok) throw new Error('Failed to load deployment info');
    return resp.json();
  },

  /** Aggregate token/cost usage dashboard. */
  async usageOverview(): Promise<UsageOverview> {
    const resp = await fetch('/api/usage');
    if (!resp.ok) throw new Error('Failed to load usage');
    return resp.json();
  },

  /** Per-conversation usage drill-down (root conversation id). */
  async usageConversationDetail(id: string): Promise<ConversationUsageDetail> {
    const resp = await fetch(`/api/usage/conversation/${encodeURIComponent(id)}`);
    if (!resp.ok) throw new Error('Failed to load conversation usage');
    return resp.json();
  },

  /** Trajectory-compatible analytics export preview for one root conversation. */
  async analyticsTrajectoryExport(id: string): Promise<TrajectoryExportPayload> {
    const resp = await fetch(`/api/analytics/conversation/${encodeURIComponent(id)}/trajectory-export`);
    if (!resp.ok) throw new Error('Failed to load analytics export');
    return resp.json();
  },

  /** Open a path's containing folder in the server host's file manager.
   * Only succeeds when the browser is on the server host (see
   * DeploymentInfo.local_access); rejects with the server's message otherwise. */
  async revealPath(path: string): Promise<void> {
    const resp = await fetch('/api/files/reveal', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path }),
    });
    if (!resp.ok) {
      let detail = 'Failed to reveal path';
      try {
        const body = (await resp.json()) as { error?: string };
        if (body.error) detail = body.error;
      } catch {
        // non-JSON error body; keep the generic message
      }
      throw new Error(detail);
    }
  },

  async getNotificationSettings(): Promise<NotificationSettings> {
    const resp = await fetch('/api/settings/notifications');
    if (!resp.ok) throw new Error('Failed to load notification settings');
    return resp.json();
  },

  async updateNotificationSettings(settings: NotificationSettings): Promise<NotificationSettings> {
    const resp = await fetch('/api/settings/notifications', {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(settings),
    });
    if (!resp.ok) throw new Error('Failed to save notification settings');
    return resp.json();
  },

  async getLlmLanguageSetting(): Promise<LlmLanguageSetting> {
    const resp = await fetch('/api/settings/llm-language');
    if (!resp.ok) throw new Error('Failed to load LLM language setting');
    return resp.json();
  },

  async updateLlmLanguageSetting(language: string): Promise<LlmLanguageSetting> {
    const resp = await fetch('/api/settings/llm-language', {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ language }),
    });
    if (!resp.ok) throw new Error('Failed to save LLM language setting');
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

  // ---- Codex / ChatGPT OAuth login (task 27104) ----

  async codexLoginPreflight(): Promise<CodexLoginPreflight> {
    const resp = await fetch('/api/codex/login/preflight');
    if (!resp.ok) throw new Error('Failed to check codex login preflight');
    return resp.json();
  },

  async codexPkceStart(): Promise<CodexPkceStartResponse> {
    const resp = await fetch('/api/codex/login/pkce/start', { method: 'POST' });
    if (!resp.ok) throw new Error('Failed to start PKCE login');
    return resp.json();
  },

  async codexPkceManual(sessionId: string, body: CodexManualCodeRequest): Promise<void> {
    const resp = await fetch(`/api/codex/login/pkce/${encodeURIComponent(sessionId)}/manual`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    });
    if (!resp.ok) {
      const err = await resp.json().catch(() => ({})) as { error?: string; message?: string };
      throw new Error(err.message ?? err.error ?? 'Manual code submission failed');
    }
  },

  async codexPkceStatus(sessionId: string): Promise<CodexLoginStatus> {
    const resp = await fetch(`/api/codex/login/pkce/${encodeURIComponent(sessionId)}/status`);
    if (!resp.ok) throw new Error('Failed to read login status');
    return resp.json();
  },

  async codexPkceCancel(sessionId: string): Promise<void> {
    await fetch(`/api/codex/login/pkce/${encodeURIComponent(sessionId)}/cancel`, { method: 'POST' });
  },

  async codexDeviceStart(): Promise<CodexDeviceStartResponse> {
    const resp = await fetch('/api/codex/login/device/start', { method: 'POST' });
    if (!resp.ok) {
      const err = await resp.json().catch(() => ({})) as { error?: string; message?: string };
      throw new Error(err.message ?? err.error ?? 'Failed to start device code login');
    }
    return resp.json();
  },

  async codexDeviceStatus(sessionId: string): Promise<CodexLoginStatus> {
    const resp = await fetch(`/api/codex/login/device/${encodeURIComponent(sessionId)}/status`);
    if (!resp.ok) throw new Error('Failed to read login status');
    return resp.json();
  },

  async codexDeviceCancel(sessionId: string): Promise<void> {
    await fetch(`/api/codex/login/device/${encodeURIComponent(sessionId)}/cancel`, { method: 'POST' });
  },

  async codexSignout(): Promise<void> {
    const resp = await fetch('/api/codex/login/signout', { method: 'POST' });
    if (!resp.ok) {
      const err = await resp.json().catch(() => ({})) as { error?: string };
      throw new Error(err.error ?? 'Sign-out failed');
    }
    const body = await resp.json().catch(() => ({})) as { ok?: boolean; error?: string };
    if (body.ok === false) throw new Error(body.error ?? 'Sign-out failed');
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
    files: File[] = [],
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
    if (files.length > 0) {
      const form = new FormData();
      form.append('metadata', new Blob([JSON.stringify(body)], { type: 'application/json' }));
      for (const file of files) form.append('files', file, file.name);
      const resp = await fetch('/api/conversations/new/with-attachments', {
        method: 'POST',
        body: form,
      });
      if (!resp.ok) {
        const err = await resp.json();
        if (resp.status === 409) {
          throw new ConflictError(err as ConflictErrorDetail);
        }
        // The first message is expanded at create time even on the attachment
        // path, so an unresolvable `@file`/`/skill` reference comes back as 422
        // (REQ-IR-007) — surface it as ExpansionError like the JSON path does.
        if (resp.status === 422) {
          throw new ExpansionError(err as ExpansionErrorDetail);
        }
        throw new Error(err.error || 'Failed to create conversation');
      }
      return (await resp.json()).conversation;
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
      // The first message is expanded at create time, same as a chat send, so
      // an unresolvable `@file` reference comes back as 422 (REQ-IR-007).
      if (resp.status === 422) {
        throw new ExpansionError(err as ExpansionErrorDetail);
      }
      throw new Error(err.error || 'Failed to create conversation');
    }
    return (await resp.json()).conversation;
  },

  async getConversation(id: string): Promise<{ conversation: Conversation; messages: Message[]; agent_working: boolean; presentation_mode: string; context_window_size: number }> {
    const resp = await fetch(`/api/conversations/${encodeURIComponent(id)}`);
    if (!resp.ok) {
      if (resp.status === 404) throw new Error('Conversation not found');
      throw new Error('Failed to get conversation');
    }
    return resp.json();
  },

  async getConversationBySlug(slug: string): Promise<{ conversation: Conversation; messages: Message[]; agent_working: boolean; presentation_mode: string; context_window_size: number }> {
    const resp = await fetch(`/api/conversations/by-slug/${encodeURIComponent(slug)}`);
    if (!resp.ok) {
      if (resp.status === 404) throw new Error('Conversation not found');
      throw new Error('Failed to get conversation');
    }
    return resp.json();
  },

  /**
   * Resolves a conversation id to its current slug. Returns null when the
   * conversation does not exist (404). Other errors throw so callers can
   * distinguish "deleted" from "transient failure" — the latter is
   * retryable.
   *
   * Uses the lightweight `/slug` endpoint instead of the full conversation
   * GET to avoid pulling the entire message history just to read one field.
   */
  async getConversationSlug(id: string): Promise<string | null> {
    const resp = await fetch(`/api/conversations/${id}/slug`);
    if (resp.status === 404) return null;
    if (!resp.ok) throw new Error('Failed to resolve conversation slug');
    return (await resp.json()).slug as string;
  },

  /** Initial pull of a scope's work-affine resource inventory (REQ-WSUI-006).
   *  The live `work_scope_update` SSE event keeps it fresh afterwards. */
  async getWorkScopeInventory(scopeKey: string): Promise<WorkScopeInventoryType> {
    const resp = await fetch(`/api/work-scope/${encodeURIComponent(scopeKey)}/inventory`);
    if (!resp.ok) throw new Error('Failed to get work-scope inventory');
    return resp.json();
  },

  /** One handle's combined inspection snapshot — identity + state, an output
   *  delta (the ring read), and a live resource sample (REQ-PINSP-005). The
   *  optional `since` is the prior response's `end_offset`; omitting it returns
   *  a recent tail (REQ-PINSP-003). The process inspector polls this while open
   *  on a live handle (REQ-PINSP-006). */
  async getBashHandleInspection(
    scopeKey: string,
    handleId: string,
    since?: number,
  ): Promise<BashHandleInspectionType> {
    const query = since !== undefined ? `?since=${encodeURIComponent(since)}` : '';
    const resp = await fetch(
      `/api/work-scope/${encodeURIComponent(scopeKey)}/bash/${encodeURIComponent(handleId)}/inspect${query}`,
    );
    // 404 = the handle table no longer knows this id (e.g. lost after a
    // restart). A typed error lets the inspector stop polling and show a
    // definitive "handle no longer exists" state instead of a transient stall.
    if (resp.status === 404) throw new NotFoundError('Bash handle no longer exists');
    if (!resp.ok) throw new Error('Failed to get bash handle inspection');
    return resp.json();
  },

  async sendMessage(
    convId: string,
    text: string,
    images: ImageData[] = [],
    files: FileAttachment[] = [],
    localId: string,
  ): Promise<{ queued: boolean; steering?: boolean }> {
    const resp = await fetch(`/api/conversations/${convId}/chat`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        text,
        images,
        files,
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

  /** Cancel a pending steering message before it is delivered.
   *  Calls `DELETE /api/conversations/:id/steering-queue/:message_id`.
   *  404 is silently ignored — the message may have already been delivered.
   */
  async uploadAttachments(convId: string, files: File[]): Promise<FileAttachment[]> {
    const form = new FormData();
    for (const file of files) form.append('files', file, file.name);
    const resp = await fetch(`/api/conversations/${convId}/attachments`, {
      method: 'POST',
      body: form,
    });
    if (!resp.ok) {
      const err = await resp.json().catch(() => ({}));
      throw new Error(err.error || 'Failed to upload attachments');
    }
    return (await resp.json()).files;
  },

  async cancelSteeringMessage(convId: string, messageId: string): Promise<void> {
    const resp = await fetch(
      `/api/conversations/${convId}/steering-queue/${messageId}`,
      { method: 'DELETE' },
    );
    if (resp.status === 404) return; // already delivered or never queued
    if (!resp.ok) throw new Error('Failed to cancel steering message');
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

  async getLocalServices(): Promise<DiscoveryServicesResponse> {
    const resp = await fetch('/api/discovery/services');
    if (!resp.ok) throw new Error('Failed to load local services');
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

  /**
   * List skills available for a working directory before a conversation exists
   * (the new-conversation composer). Directory-scoped sibling of
   * {@link listConversationSkills} (REQ-IR-005).
   */
  async listProjectSkills(
    cwd: string,
    opts?: ProjectResolutionOpts,
    signal?: AbortSignal,
  ): Promise<{ skills: SkillEntry[] }> {
    const params = new URLSearchParams({ cwd });
    applyResolutionOpts(params, opts);
    const resp = await fetch(
      `/api/skills?${params}`,
      signal ? { signal } : {},
    );
    if (!resp.ok) throw new Error('Failed to list skills');
    return resp.json();
  },

  /** List tasks from a project tasks/ directory before a conversation exists */
  async listProjectTasks(cwd: string, signal?: AbortSignal): Promise<{ tasks: TaskEntry[] }> {
    const resp = await fetch(
      `/api/tasks?cwd=${encodeURIComponent(cwd)}`,
      signal ? { signal } : {},
    );
    if (!resp.ok) throw new Error('Failed to list tasks');
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

  /** Lightweight task status counts for the conversation's project, for the
   *  collapsed Tasks header without fetching the full list. */
  async getConversationTaskCount(
    convId: string,
    currentTaskId?: string,
    signal?: AbortSignal,
  ): Promise<TaskCountResponse> {
    const params = currentTaskId
      ? `?current_task_id=${encodeURIComponent(currentTaskId)}`
      : '';
    const resp = await fetch(
      `/api/conversations/${convId}/tasks/count${params}`,
      signal ? { signal } : {},
    );
    if (!resp.ok) throw new Error('Failed to fetch task counts');
    return resp.json();
  },

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

  /**
   * Search files in a working directory before a conversation exists (the
   * new-conversation composer). Directory-scoped sibling of
   * {@link searchConversationFiles} (REQ-IR-004).
   */
  async searchProjectFiles(
    cwd: string,
    query: string,
    limit = 50,
    opts?: ProjectResolutionOpts,
    signal?: AbortSignal,
  ): Promise<{ items: FileSearchEntry[] }> {
    const params = new URLSearchParams({ cwd, q: query, limit: String(limit) });
    applyResolutionOpts(params, opts);
    const resp = await fetch(
      `/api/files/search?${params}`,
      signal ? { signal } : {},
    );
    if (!resp.ok) throw new Error('Failed to search files');
    return resp.json();
  },

  async searchConversationCode(
    convId: string,
    query: string,
    limit = 50,
    signal?: AbortSignal,
  ): Promise<{ items: CodeSearchEntry[] }> {
    const params = new URLSearchParams({ q: query, limit: String(limit) });
    const resp = await fetch(
      `/api/conversations/${convId}/code/search?${params}`,
      signal ? { signal } : {},
    );
    if (!resp.ok) throw new Error('Failed to search code');
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

  async getPrStatus(conversationId: string): Promise<PrStatusResponse> {
    const resp = await fetch(`/api/conversations/${conversationId}/pr-status`);
    if (!resp.ok) { const err = await resp.json(); throw new Error(err.error || 'Failed to fetch PR status'); }
    return resp.json();
  },

  async createPrAutoFixContext(conversationId: string): Promise<PrAutoFixContextResponse> {
    const resp = await fetch(`/api/conversations/${conversationId}/pr-auto-fix-context`, { method: 'POST' });
    if (!resp.ok) { const err = await resp.json().catch(() => ({})); throw new Error(err.error || 'Failed to capture PR context'); }
    return resp.json();
  },

  /** GET /api/conversations/:id/diff — committed and uncommitted changes
   *  in a Work/Branch-mode worktree, vs `origin/<base>` (preferred) or
   *  bare `<base>` for local-only repos. Used by the WorkActions
   *  "View diff" action. Diff sections are capped at 256KiB server-side;
   *  truncation_kib fields hold the original size when truncation hit. */
  async getConversationDiff(conversationId: string): Promise<{
    comparator: string;
    commit_log: string;
    committed_diff: string;
    committed_truncated_kib?: number;
    /** When true, committed_truncated_kib is a lower bound — UI should
     *  prefix the size with "≥". Set when the streaming reader hit its
     *  hard limit and killed the git child without seeing EOF. */
    committed_saturated?: boolean;
    uncommitted_diff: string;
    uncommitted_truncated_kib?: number;
    uncommitted_saturated?: boolean;
  }> {
    const resp = await fetch(`/api/conversations/${conversationId}/diff`);
    if (!resp.ok) { const err = await resp.json(); throw new Error(err.error || 'Failed to fetch diff'); }
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

  async approveTask(
    convId: string,
    handoff: 'continue_in_current_conversation' | 'start_fresh_work_conversation' = 'start_fresh_work_conversation',
  ): Promise<{ success: boolean; first_task?: boolean }> {
    const resp = await fetch(`/api/conversations/${convId}/approve-task`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ handoff }),
    });
    if (!resp.ok) { const err = await resp.json(); throw new Error(err.error || 'Failed to approve task'); }
    return resp.json();
  },

  async rejectTask(convId: string): Promise<{ success: boolean }> {
    const resp = await fetch(`/api/conversations/${convId}/reject-task`, { method: 'POST' });
    if (!resp.ok) { const err = await resp.json(); throw new Error(err.error || 'Failed to reject task'); }
    return resp.json();
  },

  async approveCommissionReview(convId: string): Promise<{ success: boolean }> {
    const resp = await fetch(`/api/conversations/${convId}/approve-commission-review`, { method: 'POST' });
    if (!resp.ok) { const err = await resp.json(); throw new Error(err.error || 'Failed to approve commission review'); }
    return resp.json();
  },

  async rejectCommissionReview(convId: string): Promise<{ success: boolean }> {
    const resp = await fetch(`/api/conversations/${convId}/reject-commission-review`, { method: 'POST' });
    if (!resp.ok) { const err = await resp.json(); throw new Error(err.error || 'Failed to reject commission review'); }
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

  async dismissQuestion(convId: string): Promise<{ success: boolean }> {
    const resp = await fetch(`/api/conversations/${convId}/dismiss-question`, {
      method: 'POST',
    });
    if (!resp.ok) { const err = await resp.json(); throw new Error(err.error || 'Failed to dismiss question'); }
    return resp.json();
  },

  async dismissError(convId: string): Promise<{ success: boolean }> {
    const resp = await fetch(`/api/conversations/${convId}/dismiss-error`, {
      method: 'POST',
    });
    if (!resp.ok) { const err = await resp.json(); throw new Error(err.error || 'Failed to dismiss error'); }
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
    presentation_mode: string;
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

  /** POST /api/chains/:rootId/regenerate-name — re-summarize the chain's
   *  content into a fresh name and persist it. No request body; the root id
   *  is the only input. Returns the refreshed `ChainView` (same shape as
   *  `setChainName`). On any failure the server leaves the name unchanged. */
  async regenerateChainName(rootId: string): Promise<ChainViewType> {
    const resp = await fetch(
      `/api/chains/${encodeURIComponent(rootId)}/regenerate-name`,
      { method: 'POST' },
    );
    if (resp.status === 404) throw new Error('Chain not found');
    if (!resp.ok) {
      const err = await resp.json().catch(() => ({}));
      throw new Error(err.error || 'Failed to regenerate chain name');
    }
    return resp.json();
  },

  /** POST /api/chains/:rootId/archive — archive every member of the chain
   *  atomically. Mirrors `archiveConversation` for chain-scope. */
  async archiveChain(rootId: string): Promise<void> {
    const resp = await fetch(`/api/chains/${encodeURIComponent(rootId)}/archive`, {
      method: 'POST',
    });
    if (!resp.ok) {
      const err = await resp.json().catch(() => ({}));
      throw new Error(err.error || 'Failed to archive chain');
    }
  },

  /** DELETE /api/chains/:rootId — hard-delete every member of the chain.
   *  Refused atomically (no partial wipe) if any member is busy. */
  async deleteChain(rootId: string): Promise<void> {
    const resp = await fetch(`/api/chains/${encodeURIComponent(rootId)}`, {
      method: 'DELETE',
    });
    if (!resp.ok) {
      const err = await resp.json().catch(() => ({}));
      if (resp.status === 409) {
        throw new ConflictError(err as ConflictErrorDetail);
      }
      throw new Error(err.error || 'Failed to delete chain');
    }
  },

  // -----------------------------------------------------------------
  // Decoupled task fork proposals (REQ-PROJ-034 / 037)
  //
  // The proposal id rides the existing tool-result `display_data`
  // (`fork_proposal_id`); these endpoints anchor the Review affordance and
  // its three actions to a specific proposal. A 409 means the proposal was
  // already resolved (e.g. another tab acted) — callers refetch the list and
  // let the affordance withdraw.
  // -----------------------------------------------------------------

  /** GET /api/conversations/:id/proposals — the conversation's fork proposals.
   *  The Review affordance cross-references this by `display_data.fork_proposal_id`
   *  to read each proposal's current `status`. */
  async listForkProposals(convId: string): Promise<ForkProposalSummary[]> {
    const resp = await fetch(`/api/conversations/${encodeURIComponent(convId)}/proposals`);
    if (!resp.ok) {
      if (resp.status === 404) throw new Error('Conversation not found');
      throw new Error('Failed to list fork proposals');
    }
    return (await resp.json()).proposals as ForkProposalSummary[];
  },

  /** POST /proposals/:proposalId/approve — spawn the Work fork (REQ-PROJ-034).
   *  Returns the new fork conversation's id. 409 ⇒ already resolved. */
  async approveForkProposal(
    convId: string,
    proposalId: string,
  ): Promise<{ fork_conversation_id: string }> {
    const resp = await fetch(
      `/api/conversations/${encodeURIComponent(convId)}/proposals/${encodeURIComponent(proposalId)}/approve`,
      { method: 'POST' },
    );
    if (!resp.ok) {
      const err = await resp.json().catch(() => ({}));
      if (resp.status === 409) throw new ConflictError(err as ConflictErrorDetail);
      throw new Error(err.error || 'Failed to approve proposal');
    }
    return resp.json();
  },

  /** POST /proposals/:proposalId/dismiss — record a `dismissed` resolution
   *  (REQ-PROJ-034). Idempotent: `no_op` is true when already resolved. */
  async dismissForkProposal(
    convId: string,
    proposalId: string,
  ): Promise<{ success: boolean; no_op?: boolean }> {
    const resp = await fetch(
      `/api/conversations/${encodeURIComponent(convId)}/proposals/${encodeURIComponent(proposalId)}/dismiss`,
      { method: 'POST' },
    );
    if (!resp.ok) {
      const err = await resp.json().catch(() => ({}));
      if (resp.status === 409) throw new ConflictError(err as ConflictErrorDetail);
      throw new Error(err.error || 'Failed to dismiss proposal');
    }
    return resp.json();
  },

  /** POST /proposals/:proposalId/request-changes — promote the proposal to a
   *  fresh Explore refinement carrying the user's change-request note
   *  (REQ-PROJ-037). Returns the refinement conversation's id. 409 ⇒ already
   *  resolved. */
  async requestChangesForkProposal(
    convId: string,
    proposalId: string,
    note: string,
  ): Promise<{ refinement_conversation_id: string }> {
    const resp = await fetch(
      `/api/conversations/${encodeURIComponent(convId)}/proposals/${encodeURIComponent(proposalId)}/request-changes`,
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ note }),
      },
    );
    if (!resp.ok) {
      const err = await resp.json().catch(() => ({}));
      if (resp.status === 409) throw new ConflictError(err as ConflictErrorDetail);
      throw new Error(err.error || 'Failed to request changes');
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
        // Runtime shape is guaranteed by valibot; the discriminated union of
        // ChainSseEventData requires each `type` variant to match its data
        // shape, which TS can't infer through the generic `handle` wrapper.
        onEvent({ type: eventName, ...parsed } as unknown as ChainSseEventData);
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
