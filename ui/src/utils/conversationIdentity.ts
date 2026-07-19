import type { Conversation, Project } from '../api';

export interface ConversationIdentityDisplay {
  title: string;
  projectLabel: string | null;
}

const UUID_PATTERN = /[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}/i;
const LONG_HEX_TOKEN_PATTERN = /(?:^|[-_])[0-9a-f]{24,}(?:$|[-_])/i;
const PHOENIX_WORKTREE_SEGMENT = '.phoenix/worktrees/';

function pathLeaf(path: string | null | undefined): string | null {
  const normalized = path?.trim().replace(/\/+$/, '');
  if (!normalized) return null;
  const leaf = normalized.split('/').filter(Boolean).pop();
  return leaf || null;
}

export function isLowValueIdentifier(value: string | null | undefined): boolean {
  const normalized = value?.trim();
  if (!normalized) return true;
  return UUID_PATTERN.test(normalized)
    || /^[0-9a-f]{24,}$/i.test(normalized)
    || LONG_HEX_TOKEN_PATTERN.test(normalized);
}

function projectLabelFromPath(path: string | null | undefined): string | null {
  const normalized = path?.trim();
  if (!normalized || normalized.includes(PHOENIX_WORKTREE_SEGMENT)) return null;
  const leaf = pathLeaf(normalized);
  return isLowValueIdentifier(leaf) ? null : leaf;
}

export function getProjectDisplayLabel(project: Pick<Project, 'canonical_path'>): string | null {
  return projectLabelFromPath(project.canonical_path);
}

export function getConversationProjectLabel(conversation: Pick<Conversation, 'project_name' | 'worktree_path' | 'cwd'>): string | null {
  if (!isLowValueIdentifier(conversation.project_name)) return conversation.project_name!.trim();
  return projectLabelFromPath(conversation.worktree_path || conversation.cwd);
}

export function getConversationDisplayTitle(
  conversation: Pick<Conversation, 'slug' | 'task_title' | 'branch_name' | 'project_name' | 'worktree_path' | 'cwd'>,
  fallback = 'Untitled conversation',
): string {
  const candidates = [
    conversation.slug,
    conversation.task_title,
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
