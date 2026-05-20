import type { Conversation } from '../../api';

export function activeConversationFileRoot(conversation: Conversation | null): string | null {
  return conversation?.worktree_path ?? conversation?.cwd ?? null;
}
