// Tests for the FileExplorer adapter over the unified viewer slot.
//
// openFile/closeFile/openFileState project the slot's prose state. The URL
// contract, patchContext scoping, and REQ-VS-014 persistence/restoration live
// in ViewerSlotProvider (see ViewerSlotContext.test.tsx for the slot-level
// transitions); these tests assert the prose behavior end-to-end through the
// adapter, since that is what the file explorer panel + command palette use.

import { describe, it, expect, beforeEach } from 'vitest';
import { render, act, fireEvent } from '@testing-library/react';
import { useEffect, useState } from 'react';
import type { ReactNode } from 'react';
import {
  MemoryRouter,
  Routes,
  Route,
  useLocation,
  useNavigate,
} from 'react-router-dom';
import { FileExplorerProvider } from './FileExplorerContext';
import { ViewerSlotProvider } from '../../contexts/ViewerSlotContext';
import { useFileExplorer } from '../../hooks/useFileExplorer';
import {
  clearLastViewer,
  getLastViewer,
  setLastViewer,
} from '../../storage/lastViewerStorage';

function Providers({ scopeKey, children }: { scopeKey?: string; children: ReactNode }) {
  return (
    <ViewerSlotProvider scopeKey={scopeKey} browserSessionActive={false}>
      <FileExplorerProvider>{children}</FileExplorerProvider>
    </ViewerSlotProvider>
  );
}

function Consumer({ onCtx }: { onCtx: (ctx: ReturnType<typeof useFileExplorer>) => void }) {
  const ctx = useFileExplorer();
  useEffect(() => { onCtx(ctx); }, [ctx, onCtx]);
  return null;
}

function LocationCapture({ onSearch }: { onSearch: (s: string) => void }) {
  const location = useLocation();
  useEffect(() => { onSearch(location.search); }, [location.search, onSearch]);
  return null;
}

describe('FileExplorer adapter — URL-driven open file', () => {
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
              <Providers scopeKey="conv-A">
                <Consumer onCtx={onCtx} />
                <LocationCapture onSearch={onSearch} />
              </Providers>
            }
          />
        </Routes>
      </MemoryRouter>,
    );

    act(() => { latest!.openFile('/repo/README.md', '/repo'); });
    expect(latest!.activeFile).toBe('/repo/README.md');
    expect(latest!.openFileState).toEqual({
      path: '/repo/README.md',
      rootDir: '/repo',
    });
    expect(search).toContain('viewer=prose');
    expect(search).toContain('file=%2Frepo%2FREADME.md');
    expect(search).toContain('root=%2Frepo');

    act(() => { latest!.closeFile(); });
    expect(latest!.activeFile).toBeNull();
    expect(latest!.openFileState).toBeNull();
    expect(search).not.toContain('file=');
    expect(search).not.toContain('root=');
  });

  it('hydrates openFileState from legacy file/root URL params (no ?viewer=)', () => {
    let latest: ReturnType<typeof useFileExplorer> | null = null;
    const onCtx = (ctx: ReturnType<typeof useFileExplorer>) => { latest = ctx; };

    render(
      <MemoryRouter initialEntries={['/c/conv-A?file=%2Frepo%2FREADME.md&root=%2Frepo']}>
        <Routes>
          <Route
            path="/c/:slug"
            element={<Providers scopeKey="conv-A"><Consumer onCtx={onCtx} /></Providers>}
          />
        </Routes>
      </MemoryRouter>,
    );

    expect(latest!.activeFile).toBe('/repo/README.md');
    expect(latest!.openFileState).toEqual({ path: '/repo/README.md', rootDir: '/repo' });
  });

  it('clears patchContext when scopeKey changes; URL-driven path/rootDir survive', () => {
    let latest: ReturnType<typeof useFileExplorer> | null = null;
    let setKey: ((k: string) => void) | null = null;
    const onCtx = (ctx: ReturnType<typeof useFileExplorer>) => { latest = ctx; };

    function ScopeHarness() {
      const [scopeKey, setScopeKey] = useState('conv-A');
      setKey = setScopeKey;
      return <Providers scopeKey={scopeKey}><Consumer onCtx={onCtx} /></Providers>;
    }

    render(
      <MemoryRouter initialEntries={['/']}>
        <ScopeHarness />
      </MemoryRouter>,
    );

    act(() => {
      latest!.openFile('/repo/x.ts', '/repo', {
        modifiedLines: new Set([3, 5]),
        firstModifiedLine: 3,
      });
    });
    expect(latest!.openFileState?.path).toBe('/repo/x.ts');
    expect(latest!.openFileState?.rootDir).toBe('/repo');
    expect(latest!.openFileState?.patchContext?.firstModifiedLine).toBe(3);

    act(() => { setKey!('conv-B'); });

    expect(latest!.openFileState?.path).toBe('/repo/x.ts');
    expect(latest!.openFileState?.rootDir).toBe('/repo');
    expect(latest!.openFileState?.patchContext).toBeUndefined();
  });
});

describe('FileExplorer adapter — REQ-VS-014 last-viewer persistence', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it('writes the URL params to storage when a file is opened', () => {
    let latest: ReturnType<typeof useFileExplorer> | null = null;
    const onCtx = (ctx: ReturnType<typeof useFileExplorer>) => { latest = ctx; };

    render(
      <MemoryRouter initialEntries={['/c/conv-A']}>
        <Routes>
          <Route
            path="/c/:slug"
            element={<Providers scopeKey="conv-A"><Consumer onCtx={onCtx} /></Providers>}
          />
        </Routes>
      </MemoryRouter>,
    );

    act(() => { latest!.openFile('/repo/README.md', '/repo'); });

    const stored = getLastViewer('conv-A');
    expect(stored).not.toBeNull();
    expect(stored).toContain('file=%2Frepo%2FREADME.md');
    expect(stored).toContain('root=%2Frepo');
  });

  it('clears the storage entry when the file is closed', () => {
    let latest: ReturnType<typeof useFileExplorer> | null = null;
    const onCtx = (ctx: ReturnType<typeof useFileExplorer>) => { latest = ctx; };

    render(
      <MemoryRouter initialEntries={['/c/conv-A']}>
        <Routes>
          <Route
            path="/c/:slug"
            element={<Providers scopeKey="conv-A"><Consumer onCtx={onCtx} /></Providers>}
          />
        </Routes>
      </MemoryRouter>,
    );

    act(() => { latest!.openFile('/repo/README.md', '/repo'); });
    expect(getLastViewer('conv-A')).not.toBeNull();

    act(() => { latest!.closeFile(); });
    expect(getLastViewer('conv-A')).toBeNull();
  });

  it('restores the URL on in-app navigation when storage has an entry', () => {
    setLastViewer('conv-A', 'file=%2Frepo%2FREADME.md&root=%2Frepo');

    let search = '';
    const onSearch = (s: string) => { search = s; };

    function NavHarness() {
      const navigate = useNavigate();
      return <button data-testid="enter" onClick={() => navigate('/c/conv-A')}>enter</button>;
    }

    const { getByTestId } = render(
      <MemoryRouter initialEntries={['/']}>
        <Routes>
          <Route path="/" element={<NavHarness />} />
          <Route
            path="/c/:slug"
            element={<Providers scopeKey="conv-A"><LocationCapture onSearch={onSearch} /></Providers>}
          />
        </Routes>
      </MemoryRouter>,
    );

    act(() => { fireEvent.click(getByTestId('enter')); });

    expect(search).toContain('file=%2Frepo%2FREADME.md');
    expect(search).toContain('root=%2Frepo');
  });

  it('does NOT restore on cold reload (location.key === default)', () => {
    setLastViewer('conv-A', 'file=%2Frepo%2FREADME.md&root=%2Frepo');

    let search = '';
    const onSearch = (s: string) => { search = s; };

    render(
      <MemoryRouter initialEntries={['/c/conv-A']}>
        <Routes>
          <Route
            path="/c/:slug"
            element={<Providers scopeKey="conv-A"><LocationCapture onSearch={onSearch} /></Providers>}
          />
        </Routes>
      </MemoryRouter>,
    );

    expect(search).not.toContain('file=');
    expect(search).not.toContain('root=');
  });

  it('does NOT restore when the URL already carries viewer params', () => {
    setLastViewer('conv-A', 'file=%2Frepo%2FOLD.md&root=%2Frepo');

    let search = '';
    const onSearch = (s: string) => { search = s; };

    function NavHarness() {
      const navigate = useNavigate();
      return (
        <button data-testid="enter" onClick={() => navigate('/c/conv-A?file=%2Frepo%2FNEW.md&root=%2Frepo')}>
          enter
        </button>
      );
    }

    const { getByTestId } = render(
      <MemoryRouter initialEntries={['/']}>
        <Routes>
          <Route path="/" element={<NavHarness />} />
          <Route
            path="/c/:slug"
            element={<Providers scopeKey="conv-A"><LocationCapture onSearch={onSearch} /></Providers>}
          />
        </Routes>
      </MemoryRouter>,
    );

    act(() => { fireEvent.click(getByTestId('enter')); });

    expect(search).toContain('file=%2Frepo%2FNEW.md');
    expect(search).not.toContain('OLD.md');
  });

  it('A→B does not leak: B mounts without restoring A storage', () => {
    setLastViewer('conv-A', 'file=%2Frepo%2FA.md&root=%2Frepo');

    let search = '';
    const onSearch = (s: string) => { search = s; };

    function NavHarness() {
      const navigate = useNavigate();
      return <button data-testid="enter-B" onClick={() => navigate('/c/conv-B')}>enter B</button>;
    }

    const { getByTestId } = render(
      <MemoryRouter initialEntries={['/']}>
        <Routes>
          <Route path="/" element={<NavHarness />} />
          <Route
            path="/c/:slug"
            element={<Providers scopeKey="conv-B"><LocationCapture onSearch={onSearch} /></Providers>}
          />
        </Routes>
      </MemoryRouter>,
    );

    act(() => { fireEvent.click(getByTestId('enter-B')); });

    expect(search).not.toContain('file=');
    expect(search).not.toContain('A.md');
    expect(getLastViewer('conv-A')).toBe('file=%2Frepo%2FA.md&root=%2Frepo');

    clearLastViewer('conv-A');
  });
});
