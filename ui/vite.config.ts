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
    // Suppress Vite's chunk size warning — bundle size is enforced
    // as a hard failure in ./dev.py check (BUNDLE_LIMIT_KB = 1200).
    chunkSizeWarningLimit: 9999,
  },
});
