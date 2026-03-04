import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// API port can be overridden via VITE_API_PORT env var
const apiPort = process.env.VITE_API_PORT || '8000';

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [react()],
  server: {
    allowedHosts: true,
    proxy: {
      '/api': {
        target: `http://localhost:${apiPort}`,
        changeOrigin: true,
      },
    },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    // Bundle size guardrail. Current: ~1100KB / ~370KB gzip (2026-03-04).
    // This warns but doesn't fail the build. To track regressions,
    // run `npx vite build` and check the output.
    chunkSizeWarningLimit: 1200, // KB — raise consciously when adding large deps
  },
});
