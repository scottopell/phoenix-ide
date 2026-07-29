import { afterEach, describe, expect, it } from 'vitest';
import type { QuotaDetails } from './sseSchemas';
import {
  clearCodexQuota,
  getCodexQuotaSnapshot,
  mergeCodexQuota,
  replaceCodexQuota,
} from './codexQuota';

function quota(overrides: Partial<QuotaDetails>): QuotaDetails {
  return {
    plan_type: 'plus',
    resets_at: null,
    limit_id: 'codex',
    limit_name: null,
    primary: null,
    secondary: null,
    credits: null,
    promo_message: null,
    rate_limit_reached_type: null,
    ...overrides,
  };
}

afterEach(clearCodexQuota);

describe('Codex quota store', () => {
  it('preserves the authoritative current window when a turn snapshot omits it', () => {
    replaceCodexQuota(quota({
      primary: { used_percent: 3, window_minutes: 300, resets_at: 1_800_000_000 },
      secondary: { used_percent: 4, window_minutes: 10_080, resets_at: 1_800_500_000 },
    }));

    mergeCodexQuota(quota({
      secondary: { used_percent: 5, window_minutes: 10_080, resets_at: 1_800_500_000 },
    }));

    expect(getCodexQuotaSnapshot()?.primary?.used_percent).toBe(3);
    expect(getCodexQuotaSnapshot()?.secondary?.used_percent).toBe(5);
  });

  it('lets an authoritative snapshot clear stale quota and depletion fields', () => {
    replaceCodexQuota(quota({
      primary: { used_percent: 100, window_minutes: 300, resets_at: 1_800_000_000 },
      credits: { has_credits: false, unlimited: false, balance: null },
      rate_limit_reached_type: 'workspace_member_credits_depleted',
    }));

    replaceCodexQuota(quota({
      secondary: { used_percent: 4, window_minutes: 10_080, resets_at: 1_800_500_000 },
    }));

    expect(getCodexQuotaSnapshot()?.primary).toBeNull();
    expect(getCodexQuotaSnapshot()?.rate_limit_reached_type).toBeNull();
    expect(getCodexQuotaSnapshot()?.secondary?.used_percent).toBe(4);
  });
});
