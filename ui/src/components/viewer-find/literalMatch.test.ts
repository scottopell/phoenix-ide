import { describe, expect, it } from 'vitest';
import { findLiteralMatches } from './literalMatch';

describe('findLiteralMatches', () => {
  it('finds every literal occurrence', () => {
    expect(findLiteralMatches('alpha beta alpha', 'alpha').matches).toEqual([
      { start: 0, end: 5 },
      { start: 11, end: 16 },
    ]);
  });

  it('supports overlapping literal matches', () => {
    expect(findLiteralMatches('banana', 'ana').matches).toEqual([
      { start: 1, end: 4 },
      { start: 3, end: 6 },
    ]);
  });

  it('stays case-sensitive and returns no matches for an empty query', () => {
    expect(findLiteralMatches('Alpha alpha', 'alpha').matches).toEqual([{ start: 6, end: 11 }]);
    expect(findLiteralMatches('Alpha alpha', '').matches).toEqual([]);
  });
});
