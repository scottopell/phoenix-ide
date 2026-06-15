import { defineConfig, type Plugin } from 'vite';
import react from '@vitejs/plugin-react';
import { readFileSync, writeFileSync } from 'fs';
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

// API endpoint can be overridden via env vars.
const apiPort = process.env.VITE_API_PORT || '8000';
const apiScheme = process.env.VITE_API_SCHEME || 'http';
const proxySecure = process.env.VITE_API_PROXY_SECURE !== 'false';
// Literal IPv4 loopback, not `localhost`: the dev backend binds 127.0.0.1, and
// on hosts where Node resolves `localhost` to `::1` first the proxy would get
// connection-refused against an IPv4-only listener.
const apiTarget = `${apiScheme}://127.0.0.1:${apiPort}`;

// Dev HTTPS: when cert/key paths are provided (./dev.py wires these to Phoenix's
// auto-managed local-CA leaf), serve over TLS. Vite 8's `resolveHttpServer` then
// uses `http2.createSecureServer` with `allowHTTP1`, so the browser↔dev hop
// negotiates HTTP/2 — even with the `/api` proxy below — lifting the HTTP/1.1
// per-host connection cap to match a TLS (h2) prod deployment.
const httpsCert = process.env.VITE_HTTPS_CERT;
const httpsKey = process.env.VITE_HTTPS_KEY;
const httpsServer =
  httpsCert && httpsKey
    ? { cert: readFileSync(httpsCert), key: readFileSync(httpsKey) }
    : undefined;

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [react(), gitkeep()],
  server: {
    allowedHosts: true,
    ...(httpsServer ? { https: httpsServer } : {}),
    proxy: {
      '/api': {
        target: apiTarget,
        changeOrigin: true,
        secure: proxySecure,
        ws: true, // proxy WebSocket upgrades (needed for terminal endpoint)
        // Forward the real client so the backend's same-host check
        // (DeploymentInfo.local_access / reveal) sees the browser's address, not
        // this loopback proxy — otherwise a remote dev-mode browser would look
        // local. See crates/phoenix-ide/src/api/local_reveal.rs.
        xfwd: true,
      },
      '/preview': {
        target: apiTarget,
        changeOrigin: true,
        secure: proxySecure,
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
