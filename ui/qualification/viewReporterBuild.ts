import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { relative, resolve } from 'node:path';
import { fileURLToPath, URL } from 'node:url';
import type { Plugin } from 'vite';

const root = fileURLToPath(new URL('../../', import.meta.url));
export const stockFactory = resolve(root, 'ui/src/hooks/conversationViewReporterFactory.ts');
export const controlledFactory = resolve(root, 'ui/qualification/conversationViewReporterFactory.controlled.ts');
const sha256 = (data: string | Uint8Array) => createHash('sha256').update(data).digest('hex');
const local = (path: string) => relative(root, path).replace(/\\/g, '/');
export function viewReporterBuild(controlled = false): Plugin {
  let output = resolve(root, controlled ? 'ui/dist-controlled-functional' : 'ui/dist');
  const factory = controlled ? controlledFactory : stockFactory;
  const config = resolve(root, controlled ? 'ui/vite.controlled-functional.config.ts' : 'ui/vite.config.ts');
  let modules: Record<string, string> = {};
  return {
    name: 'phoenix-view-reporter-build',
    enforce: 'pre',
    configResolved(resolved) {
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
    writeBundle(_options, bundle) {
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
        ui_files: Object.fromEntries(Object.keys(bundle).map(name => {
          const path = resolve(output, name);
          if (!path.startsWith(`${output}/`)) throw new Error('invalid emitted artifact path');
          return [local(path), sha256(readFileSync(path))];
        })),
        admission: controlled ? 'cfgtest_loopback_preview_only' : 'stock',
        full_acceptance: false, overhead_clearance: false,
      };
      writeFileSync(resolve(root, controlled ? 'ui/controlled-functional-build.json' : 'ui/stock-natural-build.json'), JSON.stringify(manifest, null, 2) + '\n');
    },
  };
}
