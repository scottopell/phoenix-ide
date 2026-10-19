import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync, realpathSync, readdirSync, lstatSync, writeFileSync } from 'node:fs';
import { relative, resolve } from 'node:path';
import { fileURLToPath, URL } from 'node:url';
import type { Plugin } from 'vite';

const root = fileURLToPath(new URL('../../', import.meta.url));
export const stockFactory = resolve(root, 'ui/src/hooks/conversationViewReporterFactory.ts');
export const controlledFactory = resolve(root, 'ui/qualification/conversationViewReporterFactory.controlled.ts');
const sha256 = (data: string | Uint8Array) => createHash('sha256').update(data).digest('hex');
const local = (path: string) => relative(root, path).replace(/\\/g, '/');
export function viewReporterBuild(controlled = false): Plugin {
  const canonicalOutput = resolve(root, controlled ? 'ui/dist-controlled-functional' : 'ui/dist');
  let output = canonicalOutput;
  const factory = controlled ? controlledFactory : stockFactory;
  const config = resolve(root, controlled ? 'ui/vite.controlled-functional.config.ts' : 'ui/vite.config.ts');
  let modules: Record<string, string> = {};
  let configured = false;
  let writes = 0;
  let emittedHashes: Record<string, string> | undefined;
  const inventory = (directory: string): Record<string, string> => Object.fromEntries(readdirSync(directory, { withFileTypes: true }).flatMap(entry => {
    const path = resolve(directory, entry.name);
    if (lstatSync(path).isSymbolicLink()) throw new Error('artifact symlinks forbidden');
    return entry.isDirectory() ? Object.entries(inventory(path)) : [[path, sha256(readFileSync(path))]];
  }));
  return {
    name: 'phoenix-view-reporter-build',
    enforce: 'pre',
    configResolved(resolved) {
      if (resolved.command === 'build') {
        if (!resolved.configFile || realpathSync(resolved.configFile) !== realpathSync(config)) throw new Error('reporter requires exact canonical configFile');
        const rollup = resolved.build.rollupOptions.output;
        if (Array.isArray(rollup) || (rollup && (rollup.file || (rollup.dir && resolve(rollup.dir) !== output)))) throw new Error('reporter requires single exact output directory');
        configured = true;
      }
      if (controlled && (resolved.command !== 'build' || resolved.mode !== 'controlled-functional' || resolve(resolved.root, resolved.build.outDir) !== output)) throw new Error('controlled reporter requires dedicated build recipe and output');
      if (!controlled && resolved.mode === 'controlled-functional') throw new Error('controlled mode requires dedicated config');
      if (!controlled) output = resolve(resolved.root, resolved.build.outDir);
    },
    resolveId(source, importer) {
      if (controlled && source === './conversationViewReporterFactory' && importer?.replace(/\\/g, '/').endsWith('/src/hooks/useConversationViewJourney.ts')) return controlledFactory;
      if (!controlled && source.includes('conversationViewReporterFactory.controlled')) throw new Error('controlled reporter forbidden in stock graph');
      return null;
    },
    generateBundle() {
      const graph = Array.from(this.getModuleIds()).map(id => id.split('?')[0]!);
      if (controlled ? !graph.includes(controlledFactory) || graph.includes(stockFactory) : graph.includes(controlledFactory)) throw new Error('reporter graph identity mismatch');
      modules = Object.fromEntries(graph.filter(id => id.startsWith(`${root}ui/`) && !id.includes('/node_modules/') && existsSync(id)).map(id => [local(id), sha256(readFileSync(id))]));
    },
    writeBundle(options, bundle) {
      if (!configured) throw new Error('reporter manifest requires resolved build config');
      if (!controlled && output !== canonicalOutput) return;
      if (writes++ || options.file || !options.dir || resolve(options.dir) !== output) throw new Error('reporter requires single actual output directory');
      emittedHashes = Object.fromEntries(Object.keys(bundle).map(name => {
          const path = resolve(output, name);
          if (!path.startsWith(`${output}/`)) throw new Error('invalid emitted artifact path');
          const emitted = bundle[name]!;
          const expected = emitted.type === 'chunk' ? emitted.code : emitted.source;
          const actual = readFileSync(path);
          if (!actual.equals(Buffer.from(expected))) throw new Error('emitted artifact bytes do not match actual output');
          return [path, sha256(actual)];
        }));
    },
    closeBundle: {
      order: 'post',
      sequential: true,
      handler() {
        if (!emittedHashes) return;
        const expected = { ...emittedHashes };
        for (const [path, digest] of Object.entries(inventory(resolve(root, 'ui/public')))) {
          const destination = resolve(output, relative(resolve(root, 'ui/public'), path));
          if (destination in expected) throw new Error('public asset collides with emitted artifact');
          expected[destination] = digest;
        }
        expected[resolve(output, '.gitkeep')] = sha256('');
        const actual = inventory(output);
        if (JSON.stringify(Object.keys(actual).sort()) !== JSON.stringify(Object.keys(expected).sort()) || Object.entries(expected).some(([path, digest]) => actual[path] !== digest)) throw new Error('final artifact inventory or bytes mismatch');
        const git = (args: string[]) => execFileSync('git', args, { cwd: root, encoding: 'utf8', timeout: 1000 }).trim();
        const dirty = git(['status', '--porcelain', '--untracked-files=normal']).length > 0;
        if (controlled && dirty) throw new Error('controlled artifact requires clean pinned source');
        const source = git(['rev-parse', 'HEAD']);
        const tree = git(['rev-parse', 'HEAD^{tree}']);
        if (!/^[a-f0-9]{40}$/.test(source) || !/^[a-f0-9]{40}$/.test(tree)) throw new Error('invalid build source identity');
        const manifest = {
          schema_version: 1,
          evidence_class: controlled ? 'controlled_functional' : 'stock_natural',
          sample_rate: controlled ? 1 : 0.25,
          source_sha: source, source_tree: tree, source_dirty: dirty,
          factory_relative: local(factory), factory_sha256: sha256(readFileSync(factory)),
          config_relative: local(config), config_sha256: sha256(readFileSync(config)),
          guard_relative: 'ui/qualification/viewReporterBuild.ts', guard_sha256: sha256(readFileSync(fileURLToPath(import.meta.url))),
          graph_scope: 'ui_source_not_dependency_provenance',
          resolved_module_files: modules,
          ui_files: Object.fromEntries(Object.entries(actual).map(([path, digest]) => [local(path), digest])),
          admission: controlled ? 'cfgtest_loopback_preview_only' : 'stock',
          full_acceptance: false, overhead_clearance: false,
        };
        writeFileSync(resolve(root, controlled ? 'ui/controlled-functional-build.json' : 'ui/stock-natural-build.json'), JSON.stringify(manifest, null, 2) + '\n');
      },
    },
  };
}
