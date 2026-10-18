import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';

// Isolated hosted-only library build through the existing pinned Vite/esbuild dependency.
export default defineConfig({
  define: {
    __PHOENIX_UI_SOURCE__: JSON.stringify('f16afb227d9336a863c0b0345960ea9a6d62b7c7'),
    __PHOENIX_UI_SOURCE_DIRTY__: 'true',
  },
  build: {
    write: false, minify: false, sourcemap: false,
    lib: { entry: fileURLToPath(new URL('./qualification/reporter-pagehide.entry.ts', import.meta.url)), formats: ['iife'], name: 'ReporterPagehideProbe' },
  },
});
