import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { __testables } from './capture-ladle-surface.mjs';

const { normalizeViewportMatrix, screenshotFileName } = __testables;

describe('capture-ladle-surface viewport helpers', () => {
  it('keeps legacy single-viewport capture behavior when no matrix is provided', () => {
    const viewport = { width: 960, height: 900 };

    expect(normalizeViewportMatrix(undefined, viewport)).toEqual([viewport]);
    expect(normalizeViewportMatrix([], viewport)).toEqual([viewport]);
    expect(screenshotFileName('shell-full', viewport)).toBe('shell-full.png');
  });

  it('normalizes a named viewport matrix and uses name-suffixed filenames', () => {
    const matrix = [
      { name: 'desktop', width: 1280, height: 900 },
      { name: 'mobile', width: 390, height: 844 },
    ];

    expect(normalizeViewportMatrix(matrix, { width: 960, height: 900 })).toEqual(matrix);
    expect(screenshotFileName('shell-full', matrix[0])).toBe('shell-full--desktop.png');
    expect(screenshotFileName('shell-full', matrix[1])).toBe('shell-full--mobile.png');
  });

  it('rejects malformed viewport-matrix entries', () => {
    expect(() => normalizeViewportMatrix([{ width: 1280, height: 900 }], { width: 960, height: 900 })).toThrow(
      'viewportMatrix[0] must include a non-empty name',
    );
    expect(() => normalizeViewportMatrix([{ name: 'broken', width: Number.NaN, height: 900 }], { width: 960, height: 900 })).toThrow(
      'viewportMatrix[0] must include finite width and height',
    );
  });
});

// Static shape check avoids executing the capture entrypoint during Vitest.
describe('capture-product-conversation entrypoint', () => {
  it('declares the deterministic desktop/mobile matrix in source', async () => {
    const source = await readFile(resolve(process.cwd(), 'scripts/capture-product-conversation.mjs'), 'utf8');
    expect(source).toContain("readyAttribute: 'data-product-conversation-fixture-ready'");
    expect(source).toContain("{ name: 'desktop', width: 1440, height: 900 }");
    expect(source).toContain("{ name: 'mobile', width: 390, height: 844 }");
    expect(source).toContain("expectedConsoleErrors: new Map([");
  });
});
