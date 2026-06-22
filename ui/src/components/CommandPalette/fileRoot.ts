import type { Conversation } from '../../api';

export function activeConversationFileRoot(conversation: Conversation | null): string | null {
  if (conversation?.archived) return null;
  return conversation?.worktree_path ?? conversation?.cwd ?? null;
}
