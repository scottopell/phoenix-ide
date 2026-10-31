import { api } from './api';
import { getCodexQuotaGeneration, setCodexQuota } from './codexQuota';
import type { CodexResetCredits } from './generated/CodexResetCredits';

/** One choosable reset (REQ-CRR-006). `creditId: null` lets the backend choose. */
export interface ResetOption {
  creditId: string | null;
  name: string;
  detail: string | null;
  description: string;
}

const DEFAULT_NAME = 'Full reset';
const DEFAULT_DESCRIPTION = 'Reset your current usage limits';

function nonBlank(value: string | null): string | null {
  const trimmed = value?.trim();
  return trimmed ? trimmed : null;
}

export function formatResetExpiry(expiresAt: number | null): string {
  if (expiresAt === null) return 'Does not expire';
  const date = new Date(expiresAt * 1000);
  if (Number.isNaN(date.getTime())) return 'Expiration unavailable';
  return `Expires ${date.toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    year: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
  })}`;
}

/**
 * Available credits, soonest expiry first, non-expiring last, capped at the
 * available count. A positive count with no available details yields one
 * backend-chooses option. Zero count yields no options.
 */
export function resetOptions(listing: CodexResetCredits): ResetOption[] {
  const limit = Math.max(0, listing.available_count);
  if (limit === 0) return [];
  const options = listing.credits
    .filter((credit) => credit.status === 'available')
    .sort((a, b) => (a.expires_at ?? Number.MAX_SAFE_INTEGER) - (b.expires_at ?? Number.MAX_SAFE_INTEGER))
    .slice(0, limit)
    .map((credit): ResetOption => ({
      creditId: credit.id,
      name: nonBlank(credit.title) ?? DEFAULT_NAME,
      detail: formatResetExpiry(credit.expires_at),
      description: nonBlank(credit.description) ?? DEFAULT_DESCRIPTION,
    }));
  if (options.length > 0) return options;
  return [{ creditId: null, name: DEFAULT_NAME, detail: null, description: DEFAULT_DESCRIPTION }];
}

export function resetCountLabel(count: number): string {
  return `${count} ${count === 1 ? 'reset' : 'resets'} available`;
}

/** Refresh the shared quota store after a redemption (REQ-CRR-007). */
export async function refreshCodexQuota(): Promise<void> {
  const startedAt = getCodexQuotaGeneration();
  try {
    const quota = await api.codexQuota();
    if (quota && getCodexQuotaGeneration() === startedAt) setCodexQuota(quota);
  } catch {
    // The quota display keeps its last snapshot; the next read refreshes it.
  }
}
