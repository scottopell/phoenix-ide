import { execFileSync, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const root = resolve(import.meta.dirname, '../..');
const manifest = JSON.parse(readFileSync(resolve(root, 'docs/conversation-observation-source-parity.json'), 'utf8')) as {
  base: string;
  product: string;
  files: Array<{ path: string; base_blob: string | null; product_blob: string | null; actual_blob: string | null; exception?: { reason: string; diff: string } }>;
};
const git = (...args: string[]) => execFileSync('git', args, { cwd: root, encoding: 'utf8', maxBuffer: 8 * 1024 * 1024 });
const attribution = JSON.parse(readFileSync(resolve(root, 'docs/conversation-observation-attribution.json'), 'utf8')) as {
  base: string;
  files: Array<{ path: string; base_blob: string | null; actual_blob: string; diff: string; changed_functions: string[] }>;
};
const attributionPaths = [
  'crates/phoenix-db/src/product_conversation_read.rs',
  'crates/phoenix-ide/src/api/auth.rs',
  'crates/phoenix-ide/src/api/observation.rs',
  'crates/phoenix-ide/src/observation_startup.rs',
  'crates/phoenix-ide/src/observation_startup/attribution_tests.rs',
  'docs/conversation-observation-attribution.md',
  'tests/devpy/test_observation_attribution_source.py',
  'ui/tests/observationSourceParity.test.ts',
];
function tree(ref: string): Map<string, string> {
  return new Map(git('ls-tree', '-r', ref, '--', 'ui').trim().split('\n').filter(Boolean).map(line => {
    const [metadata, path] = line.split('\t');
    return [path!, metadata!.split(' ')[2]!];
  }));
}
function actualBlob(path: string): string | null {
  const absolute = resolve(root, path);
  if (!existsSync(absolute)) return null;
  const bytes = readFileSync(absolute);
  return createHash('sha1').update(`blob ${bytes.length}\0`).update(bytes).digest('hex');
}
function exactDiff(path: string, productBlob: string | null): string {
  const scratch = mkdtempSync(resolve(tmpdir(), 'observation-source-diff-'));
  try {
    const product = `${scratch}/product`;
    const actual = `${scratch}/actual`;
    writeFileSync(product, productBlob ? execFileSync('git', ['cat-file', 'blob', productBlob], { cwd: root }) : '');
    writeFileSync(actual, readFileSync(resolve(root, path)));
    const diff = spawnSync('git', ['diff', '--no-index', '--no-color', '--no-ext-diff', '--full-index', '--src-prefix=a/', '--dst-prefix=b/', '--', product, actual], { cwd: root, encoding: 'utf8' });
    if (diff.status !== 1) throw new Error(`Expected an exception diff for ${path}: ${diff.stderr}`);
    return diff.stdout.replaceAll(product.slice(1), path).replaceAll(actual.slice(1), path);
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}
function sourcePaths(dir: string): string[] {
  return readdirSync(resolve(root, dir), { withFileTypes: true }).flatMap(entry => {
    const path = `${dir}/${entry.name}`;
    return entry.isDirectory() ? sourcePaths(path) : [path];
  });
}

// The manifest contains exact bytes of each reviewed exception, not permission
// for arbitrary edits to any file named by the allowlist.
describe('observation composition source identity', () => {
  it('pins both authorities and inventories every product/base UI source blob', () => {
    expect(manifest.base).toBe('f3893088b3cb224be9403f600173d5216e3a2f6b');
    expect(manifest.product).toBe('9ee218d4a33ebd4b94ea547d60caa62134edb960');
    const base = tree(manifest.base);
    const product = tree(manifest.product);
    const paths = new Set([...base.keys(), ...product.keys(), ...sourcePaths('ui/src')]);
    expect(manifest.files.map(file => file.path)).toEqual([...paths].sort());
    for (const file of manifest.files) {
      expect(file.base_blob, file.path).toBe(base.get(file.path) ?? null);
      expect(file.product_blob, file.path).toBe(product.get(file.path) ?? null);
      expect(actualBlob(file.path), file.path).toBe(file.actual_blob);
      if (!file.exception) expect(file.actual_blob, file.path).toBe(file.product_blob);
      else {
        expect(file.actual_blob, file.path).not.toBe(file.product_blob);
        expect(file.exception.reason.length, file.path).toBeGreaterThan(0);
        expect(file.exception.diff, file.path).toBe(exactDiff(file.path, file.product_blob));
      }
    }
  });

  it('preserves the backend outside exact attribution diffs, generated types and rig support', () => {
    expect(git('diff', '--name-only', manifest.base, '--', '.', ':(exclude)ui/src', ':(exclude)ui/tests/observationSourceParity.test.ts', ':(exclude)docs/conversation-observation-source-parity.json', ':(exclude)docs/conversation-observation-composition.md', ':(exclude)docs/conversation-observation-attribution.json', ...attributionPaths.map(path => `:(exclude)${path}`)).trim()).toBe('');
    for (const file of manifest.files.filter(file => file.path.startsWith('ui/src/generated/') || file.path.startsWith('ui/scripts/'))) {
      expect(file.actual_blob, file.path).toBe(file.base_blob);
    }
  });

  it('bounds attribution to exact reviewed diffs and preserves all other 2702 UI bytes', () => {
    expect(attribution.base).toBe('2702d8389c2fc78ac2e528c876c35a1a7040d692');
    expect(attribution.files.map(file => file.path)).toEqual([...attributionPaths].sort());
    for (const file of attribution.files) {
      const entry = git('ls-tree', attribution.base, '--', file.path).trim();
      expect(file.base_blob, file.path).toBe(entry ? entry.split(/\s+/)[2] : null);
      expect(actualBlob(file.path), file.path).toBe(file.actual_blob);
      expect(exactDiff(file.path, file.base_blob), file.path).toBe(file.diff);
    }
    const pinnedUI = tree(attribution.base);
    const currentUI = git('ls-files', '--cached', '--others', '--exclude-standard', '--', 'ui').trim().split('\n').filter(Boolean).sort();
    expect(currentUI).toEqual([...pinnedUI.keys()].sort());
    for (const [path, blob] of pinnedUI) {
      if (path !== 'ui/tests/observationSourceParity.test.ts') expect(actualBlob(path), path).toBe(blob);
    }
    const changed = new Set([
      ...git('diff', '--name-only', attribution.base).trim().split('\n').filter(Boolean),
      ...git('ls-files', '--others', '--exclude-standard').trim().split('\n').filter(Boolean),
    ]);
    expect([...changed].sort()).toEqual([...attributionPaths, 'docs/conversation-observation-attribution.json'].sort());
  });

  it('permits only the enumerated existing capability adapters, never algorithm substitutions', () => {
    const adapters = [
      'ui/src/App.markdownNavigation.test.tsx', 'ui/src/App.tsx',
      'ui/src/components/ChainWorkIdentityBlock.tsx',
      'ui/src/contexts/ApplicationCapabilities.tsx', 'ui/src/conversation/ConversationProvider.tsx',
      'ui/src/generated/ApplicationCapabilities.ts', 'ui/src/generated/CapabilityAvailability.ts',
      'ui/src/generated/CoordinatorObservationTarget.ts', 'ui/src/generated/ObservationTargetResponse.ts',
      'ui/src/hooks/conversationViewLifecycle.ts', 'ui/src/hooks/useConversationViewJourney.ts',
      'ui/src/pages/PersistedObservation.test.tsx', 'ui/src/pages/PersistedObservation.tsx',
      'ui/src/pages/ProductConversationPage.tsx',
    ];
    expect(manifest.files.filter(file => file.exception && file.path.startsWith('ui/src/')).map(file => file.path)).toEqual(adapters.sort());
    for (const path of adapters.filter(path => !['ui/src/App.tsx', 'ui/src/pages/PersistedObservation.test.tsx'].includes(path))) {
      const file = manifest.files.find(file => file.path === path)!;
      expect(file.actual_blob, path).toBe(file.base_blob);
    }
    for (const path of ['ui/src/components/MessageList.tsx', 'ui/src/components/VirtualTranscript.tsx', 'ui/src/components/ConversationNavStack.tsx', 'ui/src/components/ConversationNav.tsx']) {
      const file = manifest.files.find(file => file.path === path)!;
      expect(file.actual_blob, path).toBe(file.product_blob);
      expect(file.exception, path).toBeUndefined();
    }
  });
});
