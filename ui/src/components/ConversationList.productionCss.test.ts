import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

let buildDir: string;
let productionCss: string;

beforeAll(() => {
  buildDir = mkdtempSync(join(tmpdir(), 'phoenix-product-list-css-'));
  execFileSync('corepack', ['pnpm', 'exec', 'vite', 'build', '--outDir', buildDir], {
    cwd: process.cwd(),
    stdio: 'pipe',
  });
  const cssAssets = readdirSync(join(buildDir, 'assets')).filter((file) => file.endsWith('.css'));
  if (cssAssets.length === 0) throw new Error('Production build did not emit CSS');
  productionCss = cssAssets
    .map((file) => readFileSync(join(buildDir, 'assets', file), 'utf8'))
    .join('\n');
}, 120_000);

afterAll(() => {
  rmSync(buildDir, { recursive: true, force: true });
});

function displayWinner(css: string): string {
  const rules = [...css.matchAll(/([^{}]+)\{display:(grid|flex)\}/g)].map((match, sourceOrder) => ({
    selector: match[1]!,
    value: match[2]!,
    specificity: (match[1]!.match(/\./g) ?? []).length,
    sourceOrder,
  }));
  return rules.toSorted((left, right) => (
    right.specificity - left.specificity || right.sourceOrder - left.sourceOrder
  ))[0]!.value;
}

describe('ProductConversation production CSS cascade', () => {
  it('emits the compound grid invariant and the generic mobile flex-column rule', () => {
    expect(productionCss).toMatch(/\.conv-item\.product-conversation-list-row\{[^}]*display:grid/);

    const genericRule = productionCss.match(/(?:^|})\.conv-item\{([^}]*)\}/)?.[1];
    expect(genericRule).toContain('display:flex');
    expect(genericRule).toContain('flex-direction:column');
  });

  it.each([
    ['owner before generic', '.conv-item.product-conversation-list-row{display:grid}.conv-item{display:flex}'],
    ['generic before owner', '.conv-item{display:flex}.conv-item.product-conversation-list-row{display:grid}'],
  ])('keeps grid when %s', (_name, css) => {
    expect(displayWinner(css)).toBe('grid');
  });
});
