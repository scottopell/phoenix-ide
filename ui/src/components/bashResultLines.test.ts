import { describe, expect, it } from 'vitest';
import { bashResultLineTexts } from './bashResultLines';

describe('bashResultLineTexts', () => {
  it('reads model-facing string lines', () => {
    expect(bashResultLineTexts(['a', '', 'b'])).toEqual(['a', '', 'b']);
  });

  it('reads persisted { offset, bytes } lines', () => {
    expect(bashResultLineTexts([{ offset: 4, bytes: 'a' }, { offset: 5, bytes: 'b' }])).toEqual(['a', 'b']);
  });

  it('maps malformed entries to empty text and non-arrays to no lines', () => {
    expect(bashResultLineTexts([null, 3, { offset: 1 }])).toEqual(['', '', '']);
    expect(bashResultLineTexts(undefined)).toEqual([]);
  });
});
