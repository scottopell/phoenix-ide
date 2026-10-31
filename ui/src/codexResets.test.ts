import { describe, expect, it } from 'vitest';
import { resetCountLabel, resetOptions } from './codexResets';
import type { CodexResetCredit } from './generated/CodexResetCredit';

function credit(overrides: Partial<CodexResetCredit>): CodexResetCredit {
  return {
    id: 'c',
    reset_type: 'codex_rate_limits',
    status: 'available',
    granted_at: 1_700_000_000,
    expires_at: null,
    title: null,
    description: null,
    ...overrides,
  };
}

describe('resetOptions (REQ-CRR-006)', () => {
  it('lists available credits by soonest expiry, non-expiring last, capped at the count', () => {
    const options = resetOptions({
      account_id: null,
      available_count: 2,
      credits: [
        credit({ id: 'never', expires_at: null }),
        credit({ id: 'late', expires_at: 1_800_000_000 }),
        credit({ id: 'used', status: 'redeemed', expires_at: 1_600_000_000 }),
        credit({ id: 'soon', expires_at: 1_750_000_000 }),
      ],
    });
    expect(options.map((o) => o.creditId)).toEqual(['soon', 'late']);
  });

  it('puts non-expiring credits after expiring ones', () => {
    const options = resetOptions({
      account_id: null,
      available_count: 3,
      credits: [credit({ id: 'never' }), credit({ id: 'soon', expires_at: 1_750_000_000 })],
    });
    expect(options.map((o) => o.creditId)).toEqual(['soon', 'never']);
    expect(options[1]?.detail).toBe('Does not expire');
    expect(options[0]?.detail).toMatch(/^Expires /);
  });

  it('falls back to default title and description for blank values', () => {
    const [option] = resetOptions({
      account_id: null,
      available_count: 1,
      credits: [credit({ title: '  ', description: null })],
    });
    expect(option?.name).toBe('Full reset');
    expect(option?.description).toBe('Reset your current usage limits');
  });

  it('keeps upstream title and description', () => {
    const [option] = resetOptions({
      account_id: null,
      available_count: 1,
      credits: [credit({ title: 'Full reset (Weekly + 5 hr)', description: 'Ready to redeem' })],
    });
    expect(option?.name).toBe('Full reset (Weekly + 5 hr)');
    expect(option?.description).toBe('Ready to redeem');
  });

  it('offers one backend-chooses option when the count is positive but no details are available', () => {
    const options = resetOptions({
      account_id: null,
      available_count: 1,
      credits: [credit({ status: 'redeeming' })],
    });
    expect(options).toEqual([
      { creditId: null, name: 'Full reset', detail: null, description: 'Reset your current usage limits' },
    ]);
  });

  it('offers nothing when the count is zero', () => {
    expect(resetOptions({ account_id: null, available_count: 0, credits: [credit({})] })).toEqual([]);
  });
});

describe('resetCountLabel', () => {
  it('pluralizes', () => {
    expect(resetCountLabel(1)).toBe('1 reset available');
    expect(resetCountLabel(3)).toBe('3 resets available');
  });
});
