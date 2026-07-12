import { describe, expect, it } from 'vitest';
import { findLiteralMatches } from './literalMatch';

describe('findLiteralMatches', () => {
  it('finds every literal occurrence', () => {
    expect(findLiteralMatches('alpha beta alpha', 'alpha').matches).toEqual([
      { start: 0, end: 5 },
      { start: 11, end: 16 },
    ]);
  });

  it('returns non-overlapping matches that renderers can mark exactly', () => {
    expect(findLiteralMatches('banana', 'ana').matches).toEqual([
      { start: 1, end: 4 },
    ]);
  });

  it('is case-insensitive by default', () => {
    expect(findLiteralMatches('Alpha alpha ALPHA', 'alpha').matches).toEqual([
      { start: 0, end: 5 },
      { start: 6, end: 11 },
      { start: 12, end: 17 },
    ]);
  });

  it('uses locale-independent ASCII folding', () => {
    const original = String.prototype.toLocaleLowerCase;
    String.prototype.toLocaleLowerCase = function mockedLocaleLowerCase() {
      return String(this).replaceAll('I', 'ı').toLowerCase();
    };
    try {
      expect(findLiteralMatches('INIT', 'i').matches).toEqual([
        { start: 0, end: 1 },
        { start: 2, end: 3 },
      ]);
    } finally {
      String.prototype.toLocaleLowerCase = original;
    }
  });

  it('uses Unicode-aware case folding', () => {
    expect(findLiteralMatches('CAFÉ café CaFé', 'café').matches).toEqual([
      { start: 0, end: 4 },
      { start: 5, end: 9 },
      { start: 10, end: 14 },
    ]);
  });

  it('returns source offsets even when folding changes query width', () => {
    expect(findLiteralMatches('Straße STRASSE', 'strasse').matches).toEqual([
      { start: 7, end: 14 },
    ]);
  });

  it('returns no matches for an empty query', () => {
    expect(findLiteralMatches('Alpha alpha', '').matches).toEqual([]);
  });
});
