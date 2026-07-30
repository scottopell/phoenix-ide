import { afterEach, describe, expect, it } from 'vitest';
import type { QuotaDetails } from './sseSchemas';
import {
  clearCodexQuota,
  getCodexQuotaSnapshot,
  getCodexQuotaVersion,
  mergeCodexQuota,
  replaceCodexQuota,
  replaceCodexQuotaIfVersion,
  selectCodexQuotaAccount,
} from './codexQuota';

function quota(overrides: Partial<QuotaDetails>): QuotaDetails {
  return {
    plan_type: 'plus',
    resets_at: null,
    limit_id: 'codex',
    limit_name: null,
    primary: null,
    secondary: null,
    additional_limits: [],
    credits: null,
    individual_limit: null,
    promo_message: null,
    rate_limit_reached_type: null,
    ...overrides,
  };
}

afterEach(clearCodexQuota);

describe('Codex quota store', () => {
  it('retains quota for the same account and clears it when accounts change', () => {
    selectCodexQuotaAccount('account-a');
    replaceCodexQuota(quota({
      primary: { used_percent: 3, window_minutes: 300, resets_at: 1_800_000_000 },
    }));

    selectCodexQuotaAccount('account-a');
    expect(getCodexQuotaSnapshot()?.primary?.used_percent).toBe(3);

    selectCodexQuotaAccount('account-b');
    expect(getCodexQuotaSnapshot()).toBeNull();
  });

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

  it('does not let an older account fetch overwrite a newer turn snapshot', () => {
    replaceCodexQuota(quota({}));
    const fetchVersion = getCodexQuotaVersion();
    mergeCodexQuota(quota({
      rate_limit_reached_type: 'workspace_member_credits_depleted',
    }));

    replaceCodexQuotaIfVersion(quota({
      secondary: { used_percent: 4, window_minutes: 10_080, resets_at: 1_800_500_000 },
    }), fetchVersion);

    expect(getCodexQuotaSnapshot()?.rate_limit_reached_type).toBe(
      'workspace_member_credits_depleted',
    );
  });

  it('deduplicates the first active family against the account snapshot', () => {
    replaceCodexQuota(quota({
      additional_limits: [{
        limit_name: 'family_a',
        primary: { used_percent: 7, window_minutes: 300, resets_at: 1_800_000_000 },
        secondary: null,
      }],
    }));

    mergeCodexQuota(quota({
      limit_id: 'family-a',
      primary: { used_percent: 8, window_minutes: 300, resets_at: 1_800_000_000 },
    }));

    expect(getCodexQuotaSnapshot()?.primary?.used_percent).toBe(8);
    expect(getCodexQuotaSnapshot()?.additional_limits).toEqual([]);
  });

  it('replaces rather than mixes windows when quota families change', () => {
    replaceCodexQuota(quota({}));
    mergeCodexQuota(quota({
      limit_id: 'family-a',
      primary: { used_percent: 3, window_minutes: 300, resets_at: 1_800_000_000 },
      secondary: { used_percent: 4, window_minutes: 10_080, resets_at: 1_800_500_000 },
      individual_limit: { limit: '100', used: '25', remaining_percent: 75, resets_at: 1_800_000_000 },
      additional_limits: [{
        limit_name: 'family_b',
        primary: { used_percent: 7, window_minutes: 300, resets_at: 1_800_000_000 },
        secondary: null,
      }],
    }));

    mergeCodexQuota(quota({
      limit_id: 'family-b',
      primary: { used_percent: 8, window_minutes: 300, resets_at: 1_800_000_000 },
    }));

    expect(getCodexQuotaSnapshot()?.limit_id).toBe('family-b');
    expect(getCodexQuotaSnapshot()?.primary?.used_percent).toBe(8);
    expect(getCodexQuotaSnapshot()?.secondary).toBeNull();
    expect(getCodexQuotaSnapshot()?.individual_limit?.remaining_percent).toBe(75);
    expect(getCodexQuotaSnapshot()?.additional_limits).toEqual([
      expect.objectContaining({ limit_name: 'family-a' }),
    ]);
  });

  it('rejects stale turn snapshots after sign-out', () => {
    replaceCodexQuota(quota({
      secondary: { used_percent: 4, window_minutes: 10_080, resets_at: 1_800_500_000 },
    }));
    clearCodexQuota();

    mergeCodexQuota(quota({
      secondary: { used_percent: 99, window_minutes: 10_080, resets_at: 1_800_500_000 },
    }));

    expect(getCodexQuotaSnapshot()).toBeNull();
  });

  it('clears terminal depletion when a successful turn snapshot follows it', () => {
    replaceCodexQuota(quota({
      rate_limit_reached_type: 'workspace_member_credits_depleted',
    }));

    mergeCodexQuota(quota({
      secondary: { used_percent: 4, window_minutes: 10_080, resets_at: 1_800_500_000 },
    }));

    expect(getCodexQuotaSnapshot()?.rate_limit_reached_type).toBeNull();
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
