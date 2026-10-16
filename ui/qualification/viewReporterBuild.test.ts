// @vitest-environment node

import { resolve } from 'node:path';
import { afterEach, expect, it, vi } from 'vitest';
import { createViewJourneyReporter as stock } from '../src/hooks/conversationViewReporterFactory';
import { createViewJourneyReporter as controlled } from './conversationViewReporterFactory.controlled';
import type { ViewIdentity } from '../src/hooks/conversationViewTelemetry';
import { controlledFactory, stockFactory, viewReporterBuild } from './viewReporterBuild';

const root = resolve(controlledFactory, '../..');
const stockConfig = resolve(root, 'vite.config.ts');
const controlledConfig = resolve(root, 'vite.controlled-functional.config.ts');

const identity: ViewIdentity = { route: 'ordinary', trigger: 'navigation', product_conversation_id: null, segment_id: null, standalone: false, platform: 'desktop', frontend_source: null, frontend_source_dirty: false, cache: 'unknown', message_count_bucket: 'unknown', chapter_count_bucket: 'unknown', response_size_bucket: 'unknown', reconnect_count: 0, visible: true };
afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); });

it('stock keeps natural25% while controlled reports truthful100%, both fetch late-bound without global random mutation', async () => {
  vi.spyOn(Math, 'random').mockReturnValue(0.9);
  const stockReporter = stock();
  expect(stockReporter.begin(identity)).toBeNull();
  const reporter = controlled();
  const attempt = reporter.begin(identity)!;
  expect(attempt).not.toBeNull();
  const send = vi.fn<typeof fetch>().mockResolvedValue(new Response(null, { status: 204 }));
  vi.stubGlobal('fetch', send);
  try {
    reporter.report(attempt, 'incomplete');
    expect(send).toHaveBeenCalledTimes(1);
    const options = send.mock.calls[0]![1]!;
    expect(options.keepalive).toBe(true);
    expect(JSON.parse(options.body as string).sample_rate).toBe(1);
    await vi.waitFor(() => expect(options.signal?.aborted).toBe(false));
  } finally { vi.unstubAllGlobals(); }
});

// Exercise build hooks without invoking Vite or creating an artifact.
function hooks(controlled: boolean) {
  const plugin = viewReporterBuild(controlled);
  return plugin as unknown as {
    configResolved: (config: {command: string; mode: string; root?: string; configFile?: string; build: {outDir: string; rollupOptions?: {output?: unknown}}}) => void;
    resolveId: (source: string, importer: string) => string | null;
    generateBundle: (this: {getModuleIds: () => Iterable<string>; emitFile: (asset: {source: string}) => void}, options: object, bundle: Record<string, {type: 'chunk'; code: string}>) => void;
  };
}

it('default build rejects controlled mode/path/graph; dedicated hook aliases only owning factory import', () => {
  const normal = hooks(false);
  expect(() => normal.configResolved({root, command: 'build', mode: 'controlled-functional', configFile: stockConfig, build: {outDir: 'dist', rollupOptions: {}}})).toThrow(/controlled mode requires dedicated config/);
  expect(() => normal.resolveId(controlledFactory, stockFactory)).toThrow();
  expect(() => normal.generateBundle.call({getModuleIds: () => [controlledFactory], emitFile: () => {}}, {}, {})).toThrow();
  const dedicated = hooks(true);
  expect(dedicated.resolveId('./conversationViewReporterFactory', '/root/ui/src/hooks/useConversationViewJourney.ts')).toBe(controlledFactory);
  expect(dedicated.resolveId('./conversationViewReporterFactory', '/root/ui/src/other.ts')).toBeNull();
  for (const command of ['serve', 'build']) expect(() => dedicated.configResolved({command, mode: 'production', configFile: stockConfig, build: {outDir: 'dist', rollupOptions: {}}})).toThrow();
});

it('controlled artifact requires exclusive factory graph; stock excludes controlled module', () => {
  const plugin = hooks(true);
  for (const modules of [[], [stockFactory], [stockFactory, controlledFactory]]) expect(() => plugin.generateBundle.call({getModuleIds: () => modules, emitFile: () => {}}, {}, {})).toThrow();
  expect(() => plugin.generateBundle.call({getModuleIds: () => [controlledFactory], emitFile: () => {}}, {}, {})).not.toThrow();
  expect(() => hooks(false).generateBundle.call({getModuleIds: () => [stockFactory], emitFile: () => {}}, {}, {})).not.toThrow();
});

it('controlled config admits build only at exact mode/output; stock retains ordinary build/serve behavior', () => {
  const outDir = resolve(controlledFactory, '../../dist-controlled-functional');
  const root = resolve(controlledFactory, '../..');
  const dedicated = hooks(true);
  expect(() => dedicated.configResolved({root, command: 'build', mode: 'controlled-functional', configFile: controlledConfig, build: {outDir, rollupOptions: {}}})).not.toThrow();
  expect(() => dedicated.configResolved({root, command: 'serve', mode: 'controlled-functional', configFile: controlledConfig, build: {outDir, rollupOptions: {}}})).toThrow();
  expect(() => dedicated.configResolved({root, command: 'build', mode: 'controlled-functional', configFile: stockConfig, build: {outDir: 'dist', rollupOptions: {}}})).toThrow();
  expect(() => hooks(false).configResolved({root, command: 'serve', mode: 'development', configFile: stockConfig, build: {outDir: 'dist', rollupOptions: {}}})).not.toThrow();
});

it.each(['wrong_config', 'missing_config', 'multi_output', 'output_file', 'output_override'])('dedicated build rejects %s at resolved-config boundary', mode => {
  const build = {outDir: resolve(root, 'dist-controlled-functional'), rollupOptions: {output: mode === 'multi_output' ? [{}, {}] : mode === 'output_file' ? {file: 'bad.js'} : mode === 'output_override' ? {dir: '/other'} : undefined}};
  expect(() => hooks(true).configResolved({root, configFile: mode === 'missing_config' ? undefined : mode === 'wrong_config' ? stockConfig : controlledConfig, command: 'build', mode: 'controlled-functional', build})).toThrow(mode.includes('config') ? /exact canonical configFile/ : /single exact output/);
});
