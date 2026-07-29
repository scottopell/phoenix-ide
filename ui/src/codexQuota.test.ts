import { afterEach, describe, expect, it } from 'vitest';
import type { QuotaDetails } from './sseSchemas';
import {
  clearCodexQuota,
  getCodexQuotaSnapshot,
  setCodexQuota,
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
    setCodexQuota(quota({
      primary: { used_percent: 3, window_minutes: 300, resets_at: 1_800_000_000 },
      secondary: { used_percent: 4, window_minutes: 10_080, resets_at: 1_800_500_000 },
    }));

    setCodexQuota(quota({
      secondary: { used_percent: 5, window_minutes: 10_080, resets_at: 1_800_500_000 },
    }));

    expect(getCodexQuotaSnapshot()?.primary?.used_percent).toBe(3);
    expect(getCodexQuotaSnapshot()?.secondary?.used_percent).toBe(5);
  });
});
