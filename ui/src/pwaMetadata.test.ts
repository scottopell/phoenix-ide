import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const uiRoot = resolve(import.meta.dirname, '..');

function pngDimensions(path: string): { width: number; height: number } {
  const png = readFileSync(resolve(uiRoot, path));
  expect(png.subarray(1, 4).toString()).toBe('PNG');
  return { width: png.readUInt32BE(16), height: png.readUInt32BE(20) };
}

describe('Home Screen web app metadata', () => {
  it('declares a standalone, online-first application manifest', () => {
    const manifest = JSON.parse(
      readFileSync(resolve(uiRoot, 'public/manifest.webmanifest'), 'utf8'),
    ) as {
      name: string;
      start_url: string;
      scope: string;
      display: string;
      icons: Array<{ src: string; sizes: string; type: string }>;
    };

    expect(manifest).toMatchObject({
      name: 'Phoenix IDE',
      start_url: '/',
      scope: '/',
      display: 'standalone',
    });
    expect(manifest.icons).toEqual(expect.arrayContaining([
      expect.objectContaining({ src: '/icon-192.png', sizes: '192x192', type: 'image/png' }),
      expect.objectContaining({ src: '/icon-512.png', sizes: '512x512', type: 'image/png' }),
    ]));
    expect(manifest).not.toHaveProperty('serviceworker');
  });

  it('provides correctly sized raster install assets', () => {
    expect(pngDimensions('public/apple-touch-icon.png')).toEqual({ width: 180, height: 180 });
    expect(pngDimensions('public/icon-192.png')).toEqual({ width: 192, height: 192 });
    expect(pngDimensions('public/icon-512.png')).toEqual({ width: 512, height: 512 });
  });

  it('links standards-based and Apple standalone metadata', () => {
    const html = readFileSync(resolve(uiRoot, 'index.html'), 'utf8');
    expect(html).toContain('name="apple-mobile-web-app-capable" content="yes"');
    expect(html).toContain('name="apple-mobile-web-app-status-bar-style" content="black-translucent"');
    expect(html).toContain('rel="apple-touch-icon" sizes="180x180" href="/apple-touch-icon.png"');
    expect(html).toContain('rel="manifest" href="/manifest.webmanifest"');
    expect(html).toContain('viewport-fit=cover');
  });
});
