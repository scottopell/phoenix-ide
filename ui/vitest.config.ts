import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
  test: {
    environment: 'happy-dom',
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
    setupFiles: './src/test-setup.ts',
  },
})
