import type { MetaViewerLoaderState } from './types';

/**
 * Fetch mock for the two loader-level scenarios (loading / error). MetaViewer
 * itself never fetches — only the upstream `FileViewer` loader does — so this
 * is the *only* fixture file that touches `window.fetch`, and only these two
 * scenarios mount the loader.
 *
 * `loading`: monkeypatch `fetch` to a promise that never resolves WITHOUT
 * issuing a network request. No real connection is opened, so Playwright's
 * `networkidle` still settles while the spinner stays up.
 *
 * `error`: resolve a 500 so the loader's catch path renders its "cannot load"
 * surface — the honest "this can't be rendered" state (opaque/binary files are
 * non-openable upstream and never reach MetaViewer as a payload).
 */
export function installMetaViewerLoaderFetch(loader: MetaViewerLoaderState) {
  const originalFetch = window.fetch.bind(window);
  window.fetch = async (input, init) => {
    const url =
      typeof input === 'string' ? input : input instanceof URL ? input.pathname + input.search : input.url;
    if (url.startsWith('/api/files/read')) {
      if (loader.state === 'loading') {
        // Pending forever, but no network request is made (originalFetch is not
        // called), so the page reaches networkidle with the spinner shown.
        return new Promise<Response>(() => {});
      }
      return new Response(
        JSON.stringify({ error: 'Cannot read file: binary or undecodable content' }),
        { status: 500, headers: { 'content-type': 'application/json' } },
      );
    }
    return originalFetch(input, init);
  };
  return () => {
    window.fetch = originalFetch;
  };
}
