// Tests for FileExplorerProvider's URL-driven prose-reader state.
//
// path + rootDir live in the URL search params (?file=&root=) so an iOS PWA
// cold reload restores the open file (the URL is the only state that
// survives a JS context kill). patchContext (Set<number> highlights) lives
// in scoped state and resets when the scopeKey changes.

import { describe, it, expect } from 'vitest';
import { render, act } from '@testing-library/react';
import { useEffect } from 'react';
import { MemoryRouter, Routes, Route, useLocation } from 'react-router-dom';
import { FileExplorerProvider } from './FileExplorerContext';
import { useFileExplorer } from '../../hooks/useFileExplorer';

// Tiny consumer that exposes the context via a callback so the test can
// observe state and drive openFile/closeFile imperatively.
function Consumer({ onCtx }: { onCtx: (ctx: ReturnType<typeof useFileExplorer>) => void }) {
  const ctx = useFileExplorer();
  useEffect(() => { onCtx(ctx); }, [ctx, onCtx]);
  return null;
}

// Captures the current location.search so tests can assert URL state.
function LocationCapture({ onSearch }: { onSearch: (s: string) => void }) {
  const location = useLocation();
  useEffect(() => { onSearch(location.search); }, [location.search, onSearch]);
  return null;
}

describe('FileExplorerProvider — URL-driven open file', () => {
  it('openFile writes path + root to the URL; closeFile clears them', () => {
    let latest: ReturnType<typeof useFileExplorer> | null = null;
    let search = '';
    const onCtx = (ctx: ReturnType<typeof useFileExplorer>) => { latest = ctx; };
    const onSearch = (s: string) => { search = s; };

    render(
      <MemoryRouter initialEntries={['/c/conv-A']}>
        <Routes>
          <Route
            path="/c/:slug"
            element={
              <FileExplorerProvider scopeKey="conv-A">
                <Consumer onCtx={onCtx} />
                <LocationCapture onSearch={onSearch} />
              </FileExplorerProvider>
            }
          />
        </Routes>
      </MemoryRouter>,
    );

    act(() => { latest!.openFile('/repo/README.md', '/repo'); });
    expect(latest!.activeFile).toBe('/repo/README.md');
    expect(latest!.proseReaderState).toEqual({
      path: '/repo/README.md',
      rootDir: '/repo',
    });
    expect(search).toContain('file=%2Frepo%2FREADME.md');
    expect(search).toContain('root=%2Frepo');

    act(() => { latest!.closeFile(); });
    expect(latest!.activeFile).toBeNull();
    expect(latest!.proseReaderState).toBeNull();
    expect(search).not.toContain('file=');
    expect(search).not.toContain('root=');
  });

  it('hydrates proseReaderState from initial URL search params', () => {
    let latest: ReturnType<typeof useFileExplorer> | null = null;
    const onCtx = (ctx: ReturnType<typeof useFileExplorer>) => { latest = ctx; };

    render(
      <MemoryRouter initialEntries={['/c/conv-A?file=%2Frepo%2FREADME.md&root=%2Frepo']}>
        <Routes>
          <Route
            path="/c/:slug"
            element={
              <FileExplorerProvider scopeKey="conv-A">
                <Consumer onCtx={onCtx} />
              </FileExplorerProvider>
            }
          />
        </Routes>
      </MemoryRouter>,
    );

    expect(latest!.activeFile).toBe('/repo/README.md');
    expect(latest!.proseReaderState).toEqual({
      path: '/repo/README.md',
      rootDir: '/repo',
    });
  });

  it('clears patchContext when scopeKey changes (URL-driven path stays)', () => {
    let latest: ReturnType<typeof useFileExplorer> | null = null;
    const onCtx = (ctx: ReturnType<typeof useFileExplorer>) => { latest = ctx; };

    const { rerender } = render(
      <MemoryRouter initialEntries={['/']}>
        <FileExplorerProvider scopeKey="conv-A">
          <Consumer onCtx={onCtx} />
        </FileExplorerProvider>
      </MemoryRouter>,
    );

    act(() => {
      latest!.openFile('/repo/x.ts', '/repo', {
        modifiedLines: new Set([3, 5]),
        firstModifiedLine: 3,
      });
    });
    expect(latest!.proseReaderState?.patchContext?.firstModifiedLine).toBe(3);

    rerender(
      <MemoryRouter initialEntries={['/']}>
        <FileExplorerProvider scopeKey="conv-B">
          <Consumer onCtx={onCtx} />
        </FileExplorerProvider>
      </MemoryRouter>,
    );
    // patchContext is conversation-specific and resets on scopeKey change.
    // The URL-driven path/rootDir would also be unset here because the
    // MemoryRouter is a fresh instance per render.
    expect(latest!.proseReaderState?.patchContext).toBeUndefined();
  });
});
