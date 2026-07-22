import { describe, expect, it } from 'vitest';
import { checkoutLabel } from './gitStatusPresentation';

describe('checkoutLabel', () => {
  it('includes upstream relationship for tracked branches', () => {
    expect(checkoutLabel({
      kind: 'named_branch',
      branch_name: 'feature',
      head_oid: 'abc',
      remote_status: { kind: 'tracked', remote_ref: 'origin/feature', ahead: 2, behind: 1 },
    })).toBe('feature · origin/feature · ↑2 ↓1');
  });

  it('distinguishes a matching remote from a configured upstream', () => {
    expect(checkoutLabel({
      kind: 'named_branch',
      branch_name: 'feature',
      head_oid: 'abc',
      remote_status: { kind: 'matching', remote_ref: 'origin/feature', ahead: 0, behind: 0 },
    })).toBe('feature · matching origin/feature (not upstream) · up to date');
  });

  it('surfaces unavailable upstream observation', () => {
    expect(checkoutLabel({
      kind: 'named_branch',
      branch_name: 'feature',
      head_oid: 'abc',
      remote_status: { kind: 'unavailable', reason: 'missing ref' },
    })).toBe('feature · upstream unavailable');
  });

  it('shows detached checkout identity', () => {
    expect(checkoutLabel({ kind: 'detached', head_oid: 'abcdef123456', pointing_refs: [] }))
      .toBe('detached @ abcdef1');
  });
});
