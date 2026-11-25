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
    ['bash', 0, 'bash (pending)'],
    ['bash', 2, '3 tools pending'],
    ['', 0, 'tool (pending)'],
    ['', 1, '2 tools pending'],
  ])('preserves the tool name %s with %i queued calls', (name, count, expected) => {
    const tool = { id: 'active', name, input: {} };
    expect(getStateDescription({
      type: 'tool_executing', current_tool: tool, remaining_tools: Array.from({ length: count }, () => tool),
    })).toBe(expected);
  });

  it('keeps cancellation independent of the tool name', () => {
    expect(getStateDescription({ type: 'cancelling_tool', tool_use_id: 'active', skipped_tools: [] }))
      .toBe('cancelling...');
  });
});
