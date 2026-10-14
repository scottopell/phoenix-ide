import type { Conversation } from '../../api';

export type GroundingContext =
  | { kind: 'ordinary' }
  | { kind: 'coordinator_home'; workScopeId: string | null };

export const ORDINARY_GROUNDING: GroundingContext = { kind: 'ordinary' };

export function conversationGroundingContext(
  conversation: Pick<Conversation, 'runtime_role' | 'work_scope_id'> | null,
  coordinatorRoute: boolean,
): GroundingContext {
  return coordinatorRoute || conversation?.runtime_role === 'coordinator'
    ? { kind: 'coordinator_home', workScopeId: conversation?.work_scope_id ?? null }
    : ORDINARY_GROUNDING;
}
