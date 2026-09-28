import { describe, expect, it } from 'vitest';
import { formatGitShaForDisplay } from './utils';

describe('formatGitShaForDisplay', () => {
  it('shows a 12-character prefix for a full commit SHA', () => {
    expect(formatGitShaForDisplay('0123456789abcdef0123456789abcdef01234567'))
      .toBe('0123456789ab');
  });

  it('preserves the dirty marker after abbreviating the commit SHA', () => {
    expect(formatGitShaForDisplay('0123456789abcdef0123456789abcdef01234567-dirty'))
      .toBe('0123456789ab-dirty');
  });

  it.each(['unknown', 'abc123', 'abc123-dirty', 'unknown-build-identity', 'ABCDEF0123456789ABCDEF0123456789ABCDEF01'])(
    'leaves the fallback value %s intact',
    (gitSha) => {
      expect(formatGitShaForDisplay(gitSha)).toBe(gitSha);
    },
  );
});
