import { api } from '../api';

type OwnedConversationResolution =
  | { kind: 'found'; slug: string }
  | { kind: 'missing' }
  | { kind: 'failed'; message: string }
  | { kind: 'stale' };

export async function resolveOwnedConversationTarget(
  targetConversationId: string,
  ownerGeneration: number,
  currentOwnerGeneration: () => number,
  failureMessage: string,
): Promise<OwnedConversationResolution> {
  try {
    const targetSlug = await api.getConversationSlug(targetConversationId);
    if (currentOwnerGeneration() !== ownerGeneration) return { kind: 'stale' };
    return targetSlug ? { kind: 'found', slug: targetSlug } : { kind: 'missing' };
  } catch (err) {
    if (currentOwnerGeneration() !== ownerGeneration) return { kind: 'stale' };
    return { kind: 'failed', message: err instanceof Error ? err.message : failureMessage };
  }
}
