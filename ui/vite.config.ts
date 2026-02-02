import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// API port can be overridden via VITE_API_PORT env var
const apiPort = process.env.VITE_API_PORT || '8000';

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [react()],
  server: {
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
  },
});
