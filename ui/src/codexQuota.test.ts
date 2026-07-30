import { afterEach, describe, expect, it } from 'vitest';
import type { QuotaDetails } from './sseSchemas';
import { clearCodexQuota, getCodexQuotaSnapshot, setCodexQuota } from './codexQuota';

function quota(usedPercent: number): QuotaDetails {
  return {
    plan_type: 'plus',
    resets_at: null,
    limit_id: 'codex',
    limit_name: null,
    primary: { used_percent: usedPercent, window_minutes: 300, resets_at: 1_800_000_000 },
    secondary: null,
    additional_limits: [],
    credits: null,
    individual_limit: null,
    promo_message: null,
    rate_limit_reached_type: null,
  };
}

afterEach(clearCodexQuota);

describe('Codex quota store', () => {
  it('replaces the complete account snapshot', () => {
    setCodexQuota(quota(3));
    setCodexQuota(quota(8));
    expect(getCodexQuotaSnapshot()?.primary?.used_percent).toBe(8);
  });

  it('clears the snapshot on sign-out', () => {
    setCodexQuota(quota(3));
    clearCodexQuota();
    expect(getCodexQuotaSnapshot()).toBeNull();
  });
});
