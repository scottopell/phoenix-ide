import { useEffect } from 'react';
import { act, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { useViewerFind } from './useViewerFind';

function Harness({ text, onReady, onNavigate }: {
  text: string;
  onReady: (api: ReturnType<typeof useViewerFind>) => void;
  onNavigate?: Parameters<typeof useViewerFind>[0]['onNavigate'];
}) {
  const api = useViewerFind(onNavigate ? { text, onNavigate } : { text });
  useEffect(() => { onReady(api); }, [api, onReady]);
  return (
    <div>
      <span data-testid="query">{api.query}</span>
      <span data-testid="count">{String(api.matchCount)}</span>
      <span data-testid="active">{String(api.activeIndex)}</span>
    </div>
  );
}

describe('useViewerFind', () => {
  it('opens on query, wraps navigation, and clamps after text changes', () => {
    let api: ReturnType<typeof useViewerFind> | null = null;
    const { rerender } = render(<Harness text="alpha beta alpha gamma alpha" onReady={(next) => { api = next; }} />);

    act(() => {
      api!.setQuery('alpha');
    });
    expect(screen.getByTestId('count').textContent).toBe('3');
    expect(screen.getByTestId('active').textContent).toBe('0');

    act(() => {
      api!.nextMatch();
      api!.nextMatch();
      api!.nextMatch();
    });
    expect(screen.getByTestId('active').textContent).toBe('0');

    act(() => {
      api!.previousMatch();
    });
    expect(screen.getByTestId('active').textContent).toBe('2');

    rerender(<Harness text="alpha only" onReady={(next) => { api = next; }} />);
    expect(screen.getByTestId('count').textContent).toBe('1');
    expect(screen.getByTestId('active').textContent).toBe('0');
  });

  it('bumps focusVersion when reopening an already-open find bar', () => {
    let api: ReturnType<typeof useViewerFind> | null = null;

    render(<Harness text="alpha beta alpha" onReady={(next) => { api = next; }} />);

    expect(screen.getByTestId('count').textContent).toBe('0');
    const initialFocusVersion = api!.focusVersion;

    act(() => {
      api!.open();
    });
    const openedFocusVersion = api!.focusVersion;

    act(() => {
      api!.open();
    });

    expect(openedFocusVersion).toBeGreaterThan(initialFocusVersion);
    expect(api!.focusVersion).toBeGreaterThan(openedFocusVersion);
  });

  it('notifies adapters when an open search has an active match', () => {
    const onNavigate = vi.fn();
    let api: ReturnType<typeof useViewerFind> | null = null;

    render(<Harness text="alpha beta alpha" onNavigate={onNavigate} onReady={(next) => { api = next; }} />);

    act(() => {
      api!.setQuery('alpha');
    });

    expect(onNavigate).toHaveBeenLastCalledWith(expect.objectContaining({
      query: 'alpha',
      activeIndex: 0,
    }));

    act(() => {
      api!.nextMatch();
    });

    expect(onNavigate).toHaveBeenLastCalledWith(expect.objectContaining({
      query: 'alpha',
      activeIndex: 1,
    }));
  });
});
