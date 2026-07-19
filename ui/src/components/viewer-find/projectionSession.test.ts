import { describe, expect, it } from 'vitest';
import { createMatchId } from './findSession';
import { activeSessionMatchIndex, projectionMatchesToSessionMatches } from './projectionSession';

const matches = [
  { sourceId: 'a', sourceText: 'alpha beta alpha', start: 0, end: 5, target: { blockId: 'a', line: 1 } },
  { sourceId: 'a', sourceText: 'alpha beta alpha', start: 8, end: 13, target: { blockId: 'a', line: 1 } },
  { sourceId: 'b', sourceText: 'alpha', start: 0, end: 5, target: { blockId: 'b', line: 2 } },
];

describe('projectionSession', () => {
  it('creates stable typed identities from semantic target and range', () => {
    const sessionMatches = projectionMatchesToSessionMatches(
      matches,
      (match) => `${match.target.blockId}:${match.start}:${match.end}`,
    );
    expect(sessionMatches).toEqual([
      { id: createMatchId('a:0:5'), target: { blockId: 'a', line: 1 } },
      { id: createMatchId('a:8:13'), target: { blockId: 'a', line: 1 } },
      { id: createMatchId('b:0:5'), target: { blockId: 'b', line: 2 } },
    ]);
    expect(activeSessionMatchIndex(sessionMatches, createMatchId('a:8:13'))).toBe(1);
    expect(activeSessionMatchIndex(sessionMatches, createMatchId('missing'))).toBe(-1);
  });

  it('rejects duplicate identities instead of making reconciliation ambiguous', () => {
    expect(() => projectionMatchesToSessionMatches(matches, (match) => String(match.start)))
      .toThrow('duplicate viewer find match identity: 0');
  });
});
