import { describe, expect, it } from 'vitest';
import fc from 'fast-check';
import { safePrefix, safeSuffix } from './textBounds';

const EMOJI = '😀'; // one astral code point, two UTF-16 code units

function hasLoneSurrogate(s: string): boolean {
  return /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/.test(s);
}

describe('safePrefix', () => {
  it('matches slice for text without surrogate pairs', () => {
    expect(safePrefix('abcdef', 3)).toBe('abc');
    expect(safePrefix('abc', 3)).toBe('abc');
    expect(safePrefix('abc', 10)).toBe('abc');
    expect(safePrefix('abc', 0)).toBe('');
  });

  it('backs off instead of splitting a surrogate pair', () => {
    const text = `${'x'.repeat(199)}${EMOJI} tail`;
    expect(hasLoneSurrogate(text.slice(0, 200))).toBe(true);
    expect(safePrefix(text, 200)).toBe('x'.repeat(199));
    expect(safePrefix(text, 201)).toBe(`${'x'.repeat(199)}${EMOJI}`);
  });
});

describe('safeSuffix', () => {
  it('matches slice for text without surrogate pairs', () => {
    expect(safeSuffix('abcdef', 3)).toBe('def');
    expect(safeSuffix('abc', 5)).toBe('abc');
    expect(safeSuffix('abc', 0)).toBe('');
  });

  it('backs off instead of splitting a surrogate pair', () => {
    const text = `head ${EMOJI}${'y'.repeat(36)}`;
    expect(hasLoneSurrogate(text.slice(-37))).toBe(true);
    expect(safeSuffix(text, 37)).toBe('y'.repeat(36));
  });
});

// Well-formed strings mixing ASCII, BMP and astral characters.
const mixedText = fc.array(
  fc.oneof(fc.constantFrom('a', 'é', '日', EMOJI, '🦀', '\u{10000}', ' ')),
  { maxLength: 60 },
).map((parts) => parts.join(''));

describe('properties', () => {
  it('safePrefix is a well-formed prefix within budget, at most one unit short', () => {
    fc.assert(fc.property(mixedText, fc.nat(130), (text, max) => {
      const out = safePrefix(text, max);
      expect(text.startsWith(out)).toBe(true);
      expect(out.length).toBeLessThanOrEqual(max);
      expect(hasLoneSurrogate(out)).toBe(false);
      if (text.length > max) expect(out.length).toBeGreaterThanOrEqual(max - 1);
      else expect(out).toBe(text);
    }));
  });

  it('safeSuffix is a well-formed suffix within budget, at most one unit short', () => {
    fc.assert(fc.property(mixedText, fc.nat(130), (text, max) => {
      const out = safeSuffix(text, max);
      expect(text.endsWith(out)).toBe(true);
      expect(out.length).toBeLessThanOrEqual(max);
      expect(hasLoneSurrogate(out)).toBe(false);
      if (text.length > max) expect(out.length).toBeGreaterThanOrEqual(max - 1);
      else expect(out).toBe(text);
    }));
  });
});
