// Tests for the per-scope reset behavior of FileExplorerProvider.
// See task 02703 — file viewer state must not leak across conversation
// (or any other scope) boundary.

import { describe, it, expect } from 'vitest';
import { render, act } from '@testing-library/react';
import { useEffect } from 'react';
import { FileExplorerProvider } from './FileExplorerContext';
import { useFileExplorer } from '../../hooks/useFileExplorer';

// Tiny consumer that exposes the context via a callback so the test can
// observe state and drive openFile/closeFile imperatively.
function Consumer({ onCtx }: { onCtx: (ctx: ReturnType<typeof useFileExplorer>) => void }) {
  const ctx = useFileExplorer();
  useEffect(() => { onCtx(ctx); }, [ctx, onCtx]);
  return null;
}

describe('FileExplorerProvider scopeKey reset (task 02703)', () => {
  it('clears proseReaderState when scopeKey changes', () => {
    let latest: ReturnType<typeof useFileExplorer> | null = null;
    const onCtx = (ctx: ReturnType<typeof useFileExplorer>) => { latest = ctx; };

    const { rerender } = render(
      <FileExplorerProvider scopeKey="conv-A">
        <Consumer onCtx={onCtx} />
      </FileExplorerProvider>,
    );

    // Open a file in conv-A.
    act(() => { latest!.openFile('/repo/README.md', '/repo'); });
    expect(latest!.activeFile).toBe('/repo/README.md');
    expect(latest!.proseReaderState).not.toBeNull();

    // Switch to a different scope (conv-B). The viewer must close — conv-B
    // must NOT inherit conv-A's open file.
    rerender(
      <FileExplorerProvider scopeKey="conv-B">
        <Consumer onCtx={onCtx} />
      </FileExplorerProvider>,
    );
    expect(latest!.activeFile).toBeNull();
    expect(latest!.proseReaderState).toBeNull();
  });

  it('preserves proseReaderState when scopeKey stays the same', () => {
    let latest: ReturnType<typeof useFileExplorer> | null = null;
    const onCtx = (ctx: ReturnType<typeof useFileExplorer>) => { latest = ctx; };

    const { rerender } = render(
      <FileExplorerProvider scopeKey="conv-A">
        <Consumer onCtx={onCtx} />
      </FileExplorerProvider>,
    );
    act(() => { latest!.openFile('/repo/README.md', '/repo'); });
    expect(latest!.activeFile).toBe('/repo/README.md');

    // Re-render with the same scopeKey — unrelated parent re-renders should
    // not nuke the viewer state.
    rerender(
      <FileExplorerProvider scopeKey="conv-A">
        <Consumer onCtx={onCtx} />
      </FileExplorerProvider>,
    );
    expect(latest!.activeFile).toBe('/repo/README.md');
  });

  it('starting with no scopeKey then assigning one resets', () => {
    let latest: ReturnType<typeof useFileExplorer> | null = null;
    const onCtx = (ctx: ReturnType<typeof useFileExplorer>) => { latest = ctx; };

    const { rerender } = render(
      <FileExplorerProvider>
        <Consumer onCtx={onCtx} />
      </FileExplorerProvider>,
    );
    act(() => { latest!.openFile('/x', '/'); });
    expect(latest!.activeFile).toBe('/x');

    rerender(
      <FileExplorerProvider scopeKey="conv-A">
        <Consumer onCtx={onCtx} />
      </FileExplorerProvider>,
    );
    expect(latest!.activeFile).toBeNull();
  });
});
