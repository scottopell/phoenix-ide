// @vitest-environment node
import { resolve } from 'node:path';
import { beforeEach, expect, it, vi } from 'vitest';
const writes = vi.hoisted(() => ({ dirty: false, extra: false, missingPublic: false, badPublic: false, gitkeep: true, files: [] as Array<{path: string; data: string}> }));
vi.mock('node:child_process', () => ({ execFileSync: (_cmd: string, args: string[]) => args[0] === 'status' ? writes.dirty ? ' M file' : '' : args[1] === 'HEAD' ? 'a'.repeat(40) : 'b'.repeat(40) }));
vi.mock('node:fs', async importOriginal => {
  const real = await importOriginal<typeof import('node:fs')>();
  const isOutput = (path: string) => /\/dist(?:-controlled-functional)?(?:\/|$)/.test(path);
  const publics = (dir: string): string[] => real.readdirSync(dir, {withFileTypes: true}).flatMap(entry => entry.isDirectory() ? publics(`${dir}/${entry.name}`) : [`${dir}/${entry.name}`]);
  const publicRoot = resolve(process.cwd(), 'public');
  const outputFiles = () => ['assets/app.js', ...(writes.gitkeep ? ['.gitkeep'] : []), ...publics(publicRoot).map(path => path.slice(publicRoot.length + 1)).filter(name => !writes.missingPublic || name !== 'phoenix.svg'), ...(writes.extra ? ['unexpected'] : [])];
  return { ...real,
    lstatSync: (path: string) => isOutput(path) ? {isSymbolicLink: () => false} : real.lstatSync(path),
    readdirSync: (path: string, options: object) => {
      if (!isOutput(path)) return real.readdirSync(path, options as never);
      const suffix = path.replace(/^.*\/dist(?:-controlled-functional)?\/?/, '');
      const names = new Map<string, boolean>();
      for (const file of outputFiles()) { if (!file.startsWith(suffix ? suffix + '/' : '')) continue; const rest = file.slice(suffix ? suffix.length + 1 : 0); names.set(rest.split('/')[0]!, rest.includes('/')); }
      return [...names].map(([name, directory]) => ({name, isDirectory: () => directory}));
    }, readFileSync: (path: string | URL) => isOutput(String(path)) ? (() => { const name = String(path).replace(/^.*\/dist(?:-controlled-functional)?\//, ''); return name === '.gitkeep' ? Buffer.from('') : ['assets/app.js', 'unexpected'].includes(name) ? Buffer.from('synthetic artifact') : writes.badPublic && name === 'phoenix.svg' ? Buffer.from('corrupted') : real.readFileSync(resolve(publicRoot, name)); })() : real.readFileSync(path), writeFileSync: (path: string, data: string) => { writes.files.push({path, data}); } };
});
import { controlledFactory, stockFactory, viewReporterBuild } from './viewReporterBuild';
beforeEach(() => { writes.dirty = false; writes.extra = false; writes.missingPublic = false; writes.badPublic = false; writes.gitkeep = true; writes.files.length = 0; });

it.each([false, true])('separate %s manifest hashes actual emitted files and sourcegraph outside assetroot', controlled => {
  const plugin = viewReporterBuild(controlled) as unknown as { generateBundle: (this: {getModuleIds: () => string[]}) => void; writeBundle: (options: object, bundle: object) => void; closeBundle: {handler: () => void} };
  (plugin as unknown as {configResolved: (config: object) => void}).configResolved({command: 'build', mode: controlled ? 'controlled-functional' : 'production', root: resolve(stockFactory, '../../..'), configFile: resolve(stockFactory, controlled ? '../../../vite.controlled-functional.config.ts' : '../../../vite.config.ts'), build: {outDir: controlled ? 'dist-controlled-functional' : 'dist', rollupOptions: {}}});
  plugin.generateBundle.call({getModuleIds: () => [controlled ? controlledFactory : stockFactory]}, );
  plugin.writeBundle({dir: resolve(stockFactory, controlled ? '../../../dist-controlled-functional' : '../../../dist')}, {'assets/app.js': {type: 'chunk', code: 'synthetic artifact'}});
  expect(writes.files).toHaveLength(0);
  plugin.closeBundle.handler();
  expect(writes.files).toHaveLength(1);
  const {path, data} = writes.files[0]!;
  expect(path).toMatch(controlled ? /ui\/controlled-functional-build.json$/ : /ui\/stock-natural-build.json$/);
  const receipt = JSON.parse(data);
  expect(receipt).toMatchObject({schema_version: 1, evidence_class: controlled ? 'controlled_functional' : 'stock_natural', sample_rate: controlled ? 1 : 0.25, source_sha: 'a'.repeat(40), source_tree: 'b'.repeat(40), source_dirty: false, full_acceptance: false, overhead_clearance: false});
  expect(Object.keys(receipt.ui_files)).toHaveLength(9);
  expect(Object.keys(receipt.ui_files)).toContain(controlled ? 'ui/dist-controlled-functional/.gitkeep' : 'ui/dist/.gitkeep');
  expect(Object.keys(receipt.resolved_module_files)).toContain(receipt.factory_relative);
  for (const digest of Object.values(receipt.ui_files)) expect(digest).toMatch(/^[a-f0-9]{64}$/);
});

it('controlled refuses dirty source and out-of-root artifacts without writing a receipt', () => {
  const plugin = viewReporterBuild(true) as unknown as { writeBundle: (options: object, bundle: object) => void; closeBundle: {handler: () => void} };
  (plugin as unknown as {configResolved: (config: object) => void}).configResolved({command: 'build', mode: 'controlled-functional', root: resolve(stockFactory, '../../..'), configFile: resolve(stockFactory, '../../../vite.controlled-functional.config.ts'), build: {outDir: resolve(stockFactory, '../../../dist-controlled-functional'), rollupOptions: {}}});
  const dir = resolve(stockFactory, '../../../dist-controlled-functional');
  writes.dirty = true;
  plugin.writeBundle({dir}, {'assets/app.js': {type: 'chunk', code: 'synthetic artifact'}});
  expect(() => plugin.closeBundle.handler()).toThrow(/clean pinned/);
  writes.dirty = false;
  expect(writes.files).toHaveLength(0);
});

it.each(['wrong_dir', 'outfile', 'stale_bytes', 'multiple_write'])('actual emitted output %s cannot produce misleading manifest', mode => {
  const plugin = viewReporterBuild(true) as unknown as {configResolved: (config: object) => void; writeBundle: (options: object, bundle: object) => void; closeBundle: {handler: () => void}};
  const root = resolve(stockFactory, '../../..'); const dir = resolve(root, 'dist-controlled-functional');
  plugin.configResolved({root, configFile: resolve(root, 'vite.controlled-functional.config.ts'), command: 'build', mode: 'controlled-functional', build: {outDir: dir, rollupOptions: {}}});
  const options = mode === 'wrong_dir' ? {dir: resolve(root, 'elsewhere')} : mode === 'outfile' ? {dir, file: 'other.js'} : {dir};
  const bundle = {'assets/app.js': {type: 'chunk', code: mode === 'stale_bytes' ? 'new bytes' : 'synthetic artifact'}};
  if (mode === 'multiple_write') plugin.writeBundle(options, bundle);
  expect(() => plugin.writeBundle(options, bundle)).toThrow(mode === 'stale_bytes' ? /bytes do not match/ : /single actual output/);
  expect(writes.files).toHaveLength(0);
});

it.each(['extra', 'missingPublic', 'badPublic', 'gitkeep'])('final close inventory rejects %s discrepancy without weakening asset equality', mode => {
  const root = resolve(stockFactory, '../../..'); const dir = resolve(root, 'dist-controlled-functional');
  const plugin = viewReporterBuild(true) as unknown as {configResolved: (config: object) => void; writeBundle: (options: object, bundle: object) => void; closeBundle: {handler: () => void}};
  plugin.configResolved({root, configFile: resolve(root, 'vite.controlled-functional.config.ts'), command: 'build', mode: 'controlled-functional', build: {outDir: dir, rollupOptions: {}}});
  plugin.writeBundle({dir}, {'assets/app.js': {type: 'chunk', code: 'synthetic artifact'}});
  if (mode === 'gitkeep') writes.gitkeep = false;
  else writes[mode as 'extra' | 'missingPublic' | 'badPublic'] = true;
  expect(() => plugin.closeBundle.handler()).toThrow(/inventory or bytes mismatch/);
  expect(writes.files).toHaveLength(0);
});

it('stock custom-output CSS fixture builds cannot overwrite an existing canonical manifest', () => {
  const root = resolve(stockFactory, '../../..');
  const plugin = viewReporterBuild(false) as unknown as {
    configResolved: (config: object) => void;
    generateBundle: (this: {getModuleIds: () => string[]}) => void;
    writeBundle: (options: object, bundle: object) => void;
    closeBundle: {handler: () => void};
  };
  const canonical = {
    path: resolve(root, 'stock-natural-build.json'),
    data: 'retained canonical stock manifest'
  };
  writes.files.push(canonical);
  plugin.configResolved({
    root, configFile: resolve(root, 'vite.config.ts'),
    command: 'build', mode: 'production',
    build: {outDir: '/tmp/phoenix-product-list-css-synthetic', rollupOptions: {}}
  });
  expect(() => plugin.generateBundle.call({getModuleIds: () => [controlledFactory]})).toThrow(/graph identity mismatch/);
  plugin.generateBundle.call({getModuleIds: () => [stockFactory]});
  expect(() => plugin.writeBundle(
    {dir: '/tmp/phoenix-product-list-css-synthetic'},
    {'fixture.js': {type: 'chunk', code: 'not canonical bytes'}}
  )).not.toThrow();
  plugin.closeBundle.handler();
  const controlled = viewReporterBuild(true) as unknown as {configResolved: (config: object) => void};
  expect(() => controlled.configResolved({
    root, configFile: resolve(root, 'vite.controlled-functional.config.ts'),
    command: 'build', mode: 'controlled-functional',
    build: {outDir: '/tmp/phoenix-product-list-css-synthetic', rollupOptions: {}}
  })).toThrow(/dedicated build recipe and output/);
  expect(writes.files).toEqual([canonical]);
});
