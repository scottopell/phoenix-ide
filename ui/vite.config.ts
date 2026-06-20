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

// Languages we ship TextMate grammars for in the Pierre diff/file viewer.
// `@pierre/diffs` resolves a file's language from its extension and pulls the
// grammar out of shiki's `bundledLanguages` map — a map of all ~237 languages,
// each a code-split dynamic import. Vite emits a chunk per entry (~8MB of
// grammars baked into the binary), the overwhelming majority for languages no
// one viewing code here will open. This allowlist is the set we keep; every
// other grammar chunk is pruned from the output below. A file whose language is
// pruned still renders — as plaintext, no syntax colors.
const SHIKI_KEEP_LANGS = new Set([
  'bash', 'sh', 'shell', 'shellscript', 'zsh',
  'c', 'cpp', 'csharp', 'cs',
  'css', 'scss', 'less',
  'diff',
  'docker', 'dockerfile',
  'go', 'graphql',
  'html', 'xml',
  'ini', 'toml',
  'java', 'kotlin', 'scala', 'swift',
  'javascript', 'js', 'jsx', 'typescript', 'ts', 'tsx',
  'json', 'jsonc', 'json5', 'yaml', 'yml',
  'lua',
  'make', 'makefile',
  'markdown', 'md',
  'php',
  'python', 'py',
  'ruby', 'rb',
  'rust', 'rs',
  'sql', 'vue',
]);

// Prune shiki grammar chunks for languages outside SHIKI_KEEP_LANGS.
//
// A grammar is reachable two ways: dynamically, from shiki's `bundledLanguages`
// map (the bloat — one dynamic import per language), and statically, when a kept
// grammar embeds another (vue embeds html/css/ts/…; php embeds html/sql/…). We
// must keep the static-import closure of the allowlist or a kept grammar breaks,
// and delete only grammars reachable *solely* via the dynamic map. Pruning a
// dynamic-only grammar leaves a dangling `import('…')` in shiki's lookup chunk;
// at runtime that 404s and the viewer falls back to plaintext for that file.
//
// This keys off the package boundary `@shikijs/langs/<lang>` — shiki's public
// packaging, one published module per grammar — not any shiki internal module
// shape. If a future shiki reorganizes that packaging, the match count drops to
// zero and the build fails loudly rather than silently shipping every grammar.
function shikiLangPrune(): Plugin {
  const GRAMMAR_RE = /[/\\]@shikijs[/\\]langs[/\\]dist[/\\]([^/\\]+)\.mjs$/;
  return {
    name: 'shiki-lang-prune',
    apply: 'build',
    generateBundle(_options, bundle) {
      // langId per grammar chunk, by output fileName.
      const grammarLang = new Map<string, string>();
      for (const [fileName, out] of Object.entries(bundle)) {
        if (out.type !== 'chunk' || !out.facadeModuleId) continue;
        const m = GRAMMAR_RE.exec(out.facadeModuleId);
        if (m) grammarLang.set(fileName, m[1]);
      }
      if (grammarLang.size === 0) {
        throw new Error(
          'shiki-lang-prune: matched zero @shikijs/langs grammar chunks — ' +
            'shiki packaging or bundling changed. Re-verify GRAMMAR_RE before ' +
            'trusting this prune (it would otherwise silently ship every grammar).',
        );
      }

      // Keep set: allowlisted grammars, then their static-import closure.
      const keep = new Set<string>();
      const queue: string[] = [];
      for (const [fileName, lang] of Array.from(grammarLang.entries())) {
        if (SHIKI_KEEP_LANGS.has(lang)) {
          keep.add(fileName);
          queue.push(fileName);
        }
      }
      while (queue.length > 0) {
        const out = bundle[queue.pop()!];
        if (out?.type !== 'chunk') continue;
        for (const dep of out.imports) {
          if (!keep.has(dep)) {
            keep.add(dep);
            queue.push(dep);
          }
        }
      }

      let pruned = 0;
      for (const fileName of Array.from(grammarLang.keys())) {
        if (!keep.has(fileName)) {
          delete bundle[fileName];
          pruned++;
        }
      }
      this.info(
        `pruned ${pruned} shiki grammar chunks, kept ${grammarLang.size - pruned} ` +
          `(${SHIKI_KEEP_LANGS.size} allowlisted langs + static embeds)`,
      );
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
  plugins: [react(), shikiLangPrune(), gitkeep()],
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
