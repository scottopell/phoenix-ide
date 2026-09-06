import { describe, expect, it } from 'vitest';
import { compactGitIdentity } from './gitIdentity';

describe('compactGitIdentity', () => {
  it('shows twelve characters for clean identities', () => {
    expect(compactGitIdentity('0123456789abcdef0123456789abcdef01234567')).toBe('0123456789ab');
  });

  it('retains the dirty marker after compacting the hash', () => {
    expect(compactGitIdentity('0123456789abcdef0123456789abcdef01234567-dirty')).toBe('0123456789ab-dirty');
  });
});
