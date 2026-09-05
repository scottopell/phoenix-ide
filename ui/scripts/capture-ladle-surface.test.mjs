import { describe, expect, it } from 'vitest';
import { __testables, buildLadleStoryUrl } from './capture-ladle-surface.mjs';

const { normalizeViewportMatrix, playwrightInstallArgs, screenshotFileName } = __testables;

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

  it('installs only the selected allowlisted browser engine', () => {
    expect(playwrightInstallArgs('chromium')).toEqual(['exec', 'playwright', 'install', 'chromium']);
    expect(playwrightInstallArgs('webkit')).toEqual(['exec', 'playwright', 'install', 'webkit']);
    expect(() => playwrightInstallArgs('firefox')).toThrow('Unsupported PLAYWRIGHT_BROWSER firefox');
  });

  it('builds story URLs from an external Ladle base path without string concatenation', () => {
    const url = new URL(buildLadleStoryUrl(
      'http://fixture.example:61234/qa/ladle?stale=1#old',
      'product-conversation--mobile-open',
      { fixtureTheme: 'dark', fixtureHash: '#message-target' },
    ));

    expect(url.origin).toBe('http://fixture.example:61234');
    expect(url.pathname).toBe('/qa/ladle/');
    expect(Object.fromEntries(url.searchParams)).toEqual({
      story: 'product-conversation--mobile-open',
      mode: 'preview',
      fixtureTheme: 'dark',
      fixtureHash: '#message-target',
    });
    expect(url.hash).toBe('');
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
