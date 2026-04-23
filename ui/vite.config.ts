import { defineConfig, type Plugin } from 'vite';
import react from '@vitejs/plugin-react';
import { writeFileSync } from 'fs';
import { resolve } from 'path';

// Restore .gitkeep after vite wipes dist/ so fresh worktrees compile.
function gitkeep(): Plugin {
  return {
    name: 'gitkeep',
    closeBundle() {
      writeFileSync(resolve(__dirname, 'dist/.gitkeep'), '');
    },
  };
}

// API port can be overridden via VITE_API_PORT env var
const apiPort = process.env.VITE_API_PORT || '8000';

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [react(), gitkeep()],
  server: {
    allowedHosts: true,
    proxy: {
      '/api': {
        target: `http://localhost:${apiPort}`,
        changeOrigin: true,
        ws: true, // proxy WebSocket upgrades (needed for terminal endpoint)
      },
      '/preview': {
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
  // Pre-bundle lucide-react so the dev server doesn't re-scan every icon
  // file on cold start (rule: bundle-barrel-imports).
  optimizeDeps: {
    include: ['lucide-react'],
  },
});
