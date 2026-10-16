// @vitest-environment node
import { beforeEach, expect, it, vi } from 'vitest';
const writes = vi.hoisted(() => ({ dirty: false, files: [] as Array<{path: string; data: string}> }));
vi.mock('node:child_process', () => ({ execFileSync: (_cmd: string, args: string[]) => args[0] === 'status' ? writes.dirty ? ' M file' : '' : args[1] === 'HEAD' ? 'a'.repeat(40) : 'b'.repeat(40) }));
vi.mock('node:fs', async importOriginal => {
  const real = await importOriginal<typeof import('node:fs')>();
  return { ...real, readFileSync: (path: string | URL) => String(path).includes('/dist') ? Buffer.from('synthetic artifact') : real.readFileSync(path), writeFileSync: (path: string, data: string) => { writes.files.push({path, data}); } };
});
import { controlledFactory, stockFactory, viewReporterBuild } from './viewReporterBuild';
beforeEach(() => { writes.dirty = false; writes.files.length = 0; });

it.each([false, true])('separate %s manifest hashes actual emitted files and sourcegraph outside assetroot', controlled => {
  const plugin = viewReporterBuild(controlled) as unknown as { generateBundle: (this: {getModuleIds: () => string[]}) => void; writeBundle: (options: object, bundle: object) => void };
  plugin.generateBundle.call({getModuleIds: () => [controlled ? controlledFactory : stockFactory]}, );
  plugin.writeBundle({}, {'assets/app.js': {}});
  expect(writes.files).toHaveLength(1);
  const {path, data} = writes.files[0]!;
  expect(path).toMatch(controlled ? /ui\/controlled-functional-build.json$/ : /ui\/stock-natural-build.json$/);
  const receipt = JSON.parse(data);
  expect(receipt).toMatchObject({schema_version: 1, evidence_class: controlled ? 'controlled_functional' : 'stock_natural', sample_rate: controlled ? 1 : 0.25, source_sha: 'a'.repeat(40), source_tree: 'b'.repeat(40), source_dirty: false, full_acceptance: false, overhead_clearance: false});
  expect(Object.keys(receipt.ui_files)).toEqual([controlled ? 'ui/dist-controlled-functional/assets/app.js' : 'ui/dist/assets/app.js']);
  expect(Object.keys(receipt.resolved_module_files)).toContain(receipt.factory_relative);
  for (const digest of Object.values(receipt.ui_files)) expect(digest).toMatch(/^[a-f0-9]{64}$/);
});

it('controlled refuses dirty source and out-of-root artifacts without writing a receipt', () => {
  const plugin = viewReporterBuild(true) as unknown as { writeBundle: (options: object, bundle: object) => void };
  writes.dirty = true;
  expect(() => plugin.writeBundle({}, {})).toThrow(/clean pinned/);
  writes.dirty = false;
  expect(() => plugin.writeBundle({}, {'../../escape': {}})).toThrow(/artifact path/);
  expect(writes.files).toHaveLength(0);
});
