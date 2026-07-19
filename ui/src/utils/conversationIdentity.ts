import type { Conversation, Project } from '../api';

export interface ConversationIdentityDisplay {
  title: string;
  projectLabel: string | null;
}

export interface ConversationModeIdentity {
  key: 'explore' | 'work' | 'branch' | 'direct' | 'unknown';
  label: string | null;
  title: string;
  detail: string;
  desktopDetail?: string | null;
}

export interface ConversationBranchIdentity {
  active: string | null;
  base: string | null;
}

export interface ConversationPathIdentity {
  full: string | null;
  summary: string;
}

function conversationRootPath(conversation: Pick<Conversation, 'worktree_path' | 'cwd'>): string | null {
  return conversation.worktree_path?.trim() || conversation.cwd?.trim() || null;
}

export interface ConversationIdentity {
  title: string;
  projectLabel: string | null;
  taskTitle: string | null;
  branch: ConversationBranchIdentity;
  path: ConversationPathIdentity;
  mode: ConversationModeIdentity;
  modelLabel: string;
}

const UUID_PATTERN = /[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}/i;
const LONG_HEX_TOKEN_PATTERN = /(?:^|[-_])[0-9a-f]{24,}(?:$|[-_])/i;
const PHOENIX_WORKTREE_SEGMENT = '/.phoenix/worktrees/';
const PHOENIX_SEED_WORKTREE_SEGMENT = '/.phoenix/seed-worktrees/';

function pathLeaf(path: string | null | undefined): string | null {
  const normalized = path?.trim().replace(/\/+$/, '');
  if (!normalized) return null;
  const leaf = normalized.split('/').filter(Boolean).pop();
  return leaf || null;
}

function normalizeModeKey(mode: string | null | undefined): ConversationModeIdentity['key'] {
  switch (mode?.trim().toLowerCase()) {
    case 'explore': return 'explore';
    case 'work': return 'work';
    case 'branch': return 'branch';
    case 'direct': return 'direct';
    default: return 'unknown';
  }
}

function normalizeModeLabel(mode: string | null | undefined): string | null {
  const key = normalizeModeKey(mode);
  switch (key) {
    case 'explore': return 'Explore';
    case 'work': return 'Work';
    case 'branch': return 'Branch';
    case 'direct': return 'Direct';
    default: {
      const normalized = mode?.trim();
      return normalized ? normalized.charAt(0).toUpperCase() + normalized.slice(1).toLowerCase() : null;
    }
  }
}

function modeIdentity(mode: string | null | undefined): ConversationModeIdentity {
  const key = normalizeModeKey(mode);
  const label = normalizeModeLabel(mode);
  switch (key) {
    case 'explore':
      return { key, label, title: 'Explore mode: read-only git project', detail: 'Read-only git project', desktopDetail: 'read-only' };
    case 'work':
      return { key, label, title: 'Work mode: task branch', detail: 'Task branch', desktopDetail: 'task branch' };
    case 'branch':
      return { key, label, title: 'Branch mode: existing branch', detail: 'Existing branch', desktopDetail: 'existing branch' };
    case 'direct':
      return { key, label, title: 'Direct mode: full access', detail: 'Full access', desktopDetail: 'full access' };
    default:
      return { key, label, title: 'Full access', detail: 'Full access', desktopDetail: null };
  }
}

export function isLowValueIdentifier(value: string | null | undefined): boolean {
  const normalized = value?.trim();
  if (!normalized) return true;
  return UUID_PATTERN.test(normalized)
    || /^[0-9a-f]{24,}$/i.test(normalized)
    || LONG_HEX_TOKEN_PATTERN.test(normalized);
}

export function getPathDisplayLabel(path: string | null | undefined): string | null {
  const normalized = path?.trim().replace(/\/+$/, '');
  if (!normalized) return null;

  const seedWorktreeIndex = normalized.lastIndexOf(PHOENIX_SEED_WORKTREE_SEGMENT);
  if (seedWorktreeIndex >= 0) {
    const seedLeaf = pathLeaf(normalized.slice(seedWorktreeIndex + PHOENIX_SEED_WORKTREE_SEGMENT.length));
    if (!isLowValueIdentifier(seedLeaf)) return seedLeaf;
  }

  const managedWorktreeIndex = normalized.indexOf(PHOENIX_WORKTREE_SEGMENT);
  const identityPath = managedWorktreeIndex >= 0
    ? normalized.slice(0, managedWorktreeIndex)
    : normalized;
  const leaf = pathLeaf(identityPath);
  return isLowValueIdentifier(leaf) ? null : leaf;
}

export function getProjectDisplayLabel(project: Pick<Project, 'canonical_path'>): string | null {
  return getPathDisplayLabel(project.canonical_path);
}

export function getConversationProjectLabel(conversation: Pick<Conversation, 'project_name' | 'worktree_path' | 'cwd'>): string | null {
  if (!isLowValueIdentifier(conversation.project_name)) return conversation.project_name!.trim();
  return getPathDisplayLabel(conversationRootPath(conversation));
}

export function summarizeConversationPath(path: string | null | undefined): string {
  if (!path) return '—';
  const trimmed = path.replace(/\/+$/, '');
  const parts = trimmed.split('/').filter(Boolean);
  if (parts.length <= 2) return path;
  return `…/${parts.slice(-2).join('/')}`;
}

export function getConversationDisplayTitle(
  conversation: Pick<Conversation, 'slug' | 'task_title' | 'branch_name' | 'project_name' | 'worktree_path' | 'cwd'>,
  fallback = 'Untitled conversation',
): string {
  const candidates = [
    conversation.task_title,
    conversation.slug,
    conversation.branch_name,
    getConversationProjectLabel(conversation),
  ];
  return candidates.find((candidate) => !isLowValueIdentifier(candidate))?.trim() || fallback;
}

export function getConversationIdentityDisplay(
  conversation: Pick<Conversation, 'slug' | 'task_title' | 'branch_name' | 'project_name' | 'worktree_path' | 'cwd'>,
  fallback = 'Untitled conversation',
): ConversationIdentityDisplay {
  return {
    title: getConversationDisplayTitle(conversation, fallback),
    projectLabel: getConversationProjectLabel(conversation),
  };
}

export function getConversationIdentity(
  conversation: Pick<Conversation, 'slug' | 'task_title' | 'branch_name' | 'base_branch' | 'project_name' | 'worktree_path' | 'cwd' | 'conv_mode_label' | 'model'>,
  fallback = 'Untitled conversation',
): ConversationIdentity {
  return {
    title: getConversationDisplayTitle(conversation, fallback),
    projectLabel: getConversationProjectLabel(conversation),
    taskTitle: conversation.task_title?.trim() || null,
    branch: {
      active: conversation.branch_name?.trim() || null,
      base: conversation.base_branch?.trim() || null,
    },
    path: {
      full: conversationRootPath(conversation),
      summary: summarizeConversationPath(conversationRootPath(conversation)),
    },
    mode: modeIdentity(conversation.conv_mode_label),
    modelLabel: conversation.model,
  };
}
