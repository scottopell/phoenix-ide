import { describe, expect, it } from 'vitest';
import { formatGitShaForDisplay, getStateDescription } from './utils';

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

describe('tool activity descriptions', () => {
  it.each([
    ['bash', 0, 'bash'],
    ['bash', 2, 'bash (+2 queued)'],
    ['', 0, 'tool'],
    ['', 1, 'tool (+1 queued)'],
  ])('preserves the tool name %s with %i queued calls', (name, count, expected) => {
    const tool = { id: 'active', name, input: {} };
    expect(getStateDescription({
      type: 'tool_executing', current_tool: tool, remaining_tools: Array.from({ length: count }, () => tool),
    })).toBe(expected);
  });

  it('keeps cancellation independent of the tool name', () => {
    expect(getStateDescription({ type: 'cancelling_tool', current_tool: { id: 'active', name: 'bash', input: {} } }))
      .toBe('cancelling...');
  });
});
