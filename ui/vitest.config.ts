import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

const profileDir = process.env.PHOENIX_CHECK_PROFILE_DIR
const setupFiles = ['./src/test-setup.ts']

if (profileDir) {
  setupFiles.push('./src/testCpuProfile.setup.ts')
}

const sharedTestConfig = {
  environment: 'happy-dom' as const,
  // Sandboxed-preview components (MetaViewer/HtmlViewerBody) render an
  // <iframe src="/preview/…">. happy-dom resolves the relative src against
  // the default base URL and would otherwise issue a real network load for
  // the iframe page — a live async task that hangs (and starves co-scheduled
  // tests past the 5s timeout) whenever something is listening on the base
  // host:port. Disable iframe page loading so tests never touch the network.
  environmentOptions: {
    happyDOM: {
      settings: {
        disableIframePageLoading: true,
      },
    },
  },
  globals: true,
  setupFiles,
  sequence: {
    setupFiles: 'list' as const,
  },
}

export default defineConfig({
  plugins: [react()],
  test: profileDir
    ? {
        ...sharedTestConfig,
        // Per-worker CPU profiling reads process.cpuUsage(), which requires
        // isolated OS processes (forks).
        pool: 'forks',
        // --expose-gc lets testEnvironment.test.ts prove MutationObserver survives collection.
        execArgv: ['--no-experimental-webstorage', '--expose-gc'],
      }
    : {
        ...sharedTestConfig,
        projects: [
          {
            extends: true,
            test: {
              name: 'unit',
              pool: 'threads',
              execArgv: ['--no-experimental-webstorage'],
              exclude: ['**/node_modules/**', 'src/testEnvironment.test.ts'],
            },
          },
          {
            extends: true,
            test: {
              name: 'gc-env',
              pool: 'forks',
              // --expose-gc lets testEnvironment.test.ts prove MutationObserver survives collection.
              execArgv: ['--no-experimental-webstorage', '--expose-gc'],
              include: ['src/testEnvironment.test.ts'],
            },
          },
        ],
      },
})

export { setupFiles, profileDir }
