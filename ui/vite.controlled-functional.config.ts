import { fileURLToPath } from 'node:url';
import { defineConfig, type UserConfig } from 'vite';
import base from './vite.config';
import { viewReporterBuild } from './qualification/viewReporterBuild';

const config = base as UserConfig;
export default defineConfig({
  ...config,
  plugins: [viewReporterBuild(true), ...(config.plugins ?? []).filter(plugin => !(plugin && typeof plugin === 'object' && 'name' in plugin && plugin.name === 'phoenix-view-reporter-build'))],
  build: {
    ...config.build,
    outDir: fileURLToPath(new URL('./dist-controlled-functional', import.meta.url)),
    emptyOutDir: true,
  },
});
