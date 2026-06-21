import { describe, expect, it, beforeEach, vi } from 'vitest';
import { fireEvent, render, screen, act } from '@testing-library/react';
import { useResizablePane } from './useResizablePane';

function KeyedPaneHarness({ paneKey }: { paneKey: string }) {
  const pane = useResizablePane({
    key: paneKey,
    min: 32,
    max: 800,
    defaultSize: 300,
    collapseThreshold: 60,
    defaultCollapsed: true,
  });

  return (
    <>
      <div data-testid="keyed-pane-state">
        {pane.collapsed ? 'collapsed' : String(pane.size)}
      </div>
      <button type="button" onClick={pane.expandFromCollapsed}>
        expand
      </button>
    </>
  );
}

function RightDockedPaneHarness() {
  const pane = useResizablePane({
    key: 'test-right-docked-pane-width',
    min: 360,
    max: 1200,
    defaultSize: 600,
    collapseThreshold: 280,
  });

  return (
    <>
      <div data-testid="pane-state">
        {pane.collapsed ? 'collapsed' : String(pane.size)}
      </div>
      <div
        role="separator"
        aria-label="Resize viewer pane"
        aria-valuenow={pane.collapsed ? 0 : pane.size}
        tabIndex={0}
        onPointerDown={(e) => pane.startDrag(e, 'x', true)}
        onKeyDown={(e) => {
          if (e.key === 'ArrowLeft') {
            e.preventDefault();
            pane.setSize(pane.size + 32);
          } else if (e.key === 'ArrowRight') {
            e.preventDefault();
            pane.setSize(pane.size - 32);
          }
        }}
      />
    </>
  );
}

function dragDivider(startX: number, endX: number) {
  const divider = screen.getByRole('separator', { name: 'Resize viewer pane' });

  fireEvent.pointerDown(divider, { clientX: startX, pointerId: 1 });
  fireEvent.pointerMove(divider, { clientX: endX, pointerId: 1 });
  fireEvent.pointerUp(divider, { clientX: endX, pointerId: 1 });
}

describe('useResizablePane key switching', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it('hydrates size and collapsed state from the active key', () => {
    const { rerender } = render(<KeyedPaneHarness paneKey="terminal-height:a" />);

    expect(screen.getByTestId('keyed-pane-state')).toHaveTextContent('collapsed');

    fireEvent.click(screen.getByRole('button', { name: 'expand' }));
    expect(screen.getByTestId('keyed-pane-state')).toHaveTextContent('300');
    expect(localStorage.getItem('terminal-height:a-collapsed')).toBe('false');

    act(() => {
      rerender(<KeyedPaneHarness paneKey="terminal-height:b" />);
    });
    expect(screen.getByTestId('keyed-pane-state')).toHaveTextContent('collapsed');
    expect(localStorage.getItem('terminal-height:b-collapsed')).toBeNull();

    act(() => {
      rerender(<KeyedPaneHarness paneKey="terminal-height:a" />);
    });
    expect(screen.getByTestId('keyed-pane-state')).toHaveTextContent('300');
  });
});

function LivePaneHarness({
  onRender,
  liveCalls,
}: {
  onRender: () => void;
  liveCalls: Array<[number, boolean]>;
}) {
  const pane = useResizablePane({
    key: 'test-live-pane-width',
    min: 360,
    max: 1200,
    defaultSize: 600,
    collapseThreshold: 280,
  });
  onRender();
  return (
    <>
      <div data-testid="pane-state">{pane.collapsed ? 'collapsed' : String(pane.size)}</div>
      <div
        role="separator"
        aria-label="Resize viewer pane"
        tabIndex={0}
        onPointerDown={(e) =>
          pane.startDrag(e, 'x', true, (size, collapsed) => liveCalls.push([size, collapsed]))
        }
      />
    </>
  );
}

describe('useResizablePane live-drag channel', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it('drives onLiveResize during the drag without committing React state until pointer-up', () => {
    const raf = vi.spyOn(window, 'requestAnimationFrame').mockImplementation((cb: FrameRequestCallback) => {
      cb(0);
      return 1;
    });
    try {
      const liveCalls: Array<[number, boolean]> = [];
      let renders = 0;
      render(<LivePaneHarness onRender={() => { renders += 1; }} liveCalls={liveCalls} />);
      const rendersAfterMount = renders;

      const divider = screen.getByRole('separator', { name: 'Resize viewer pane' });
      fireEvent.pointerDown(divider, { clientX: 500, pointerId: 1 });
      fireEvent.pointerMove(divider, { clientX: 300, pointerId: 1 });

      expect(liveCalls.at(-1)).toEqual([800, false]);
      expect(screen.getByTestId('pane-state')).toHaveTextContent('600');
      expect(renders).toBe(rendersAfterMount);

      fireEvent.pointerUp(divider, { clientX: 300, pointerId: 1 });
      expect(screen.getByTestId('pane-state')).toHaveTextContent('800');
      expect(renders).toBe(rendersAfterMount + 1);
    } finally {
      raf.mockRestore();
    }
  });
});

describe('useResizablePane right-docked divider semantics', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it('makes a right-hand pane wider when the divider is dragged left', () => {
    render(<RightDockedPaneHarness />);

    expect(screen.getByTestId('pane-state')).toHaveTextContent('600');

    dragDivider(500, 300);

    expect(screen.getByTestId('pane-state')).toHaveTextContent('800');
  });

  it('makes a right-hand pane narrower when the divider is dragged right', () => {
    render(<RightDockedPaneHarness />);

    dragDivider(500, 700);

    expect(screen.getByTestId('pane-state')).toHaveTextContent('400');
  });

  it('keeps right-docked pointer resizing aligned with keyboard resizing', () => {
    render(<RightDockedPaneHarness />);
    const divider = screen.getByRole('separator', { name: 'Resize viewer pane' });

    fireEvent.keyDown(divider, { key: 'ArrowLeft' });
    expect(screen.getByTestId('pane-state')).toHaveTextContent('632');

    fireEvent.keyDown(divider, { key: 'ArrowRight' });
    expect(screen.getByTestId('pane-state')).toHaveTextContent('600');
  });

  it('preserves clamp and collapse behavior while using inverted drag deltas', () => {
    render(<RightDockedPaneHarness />);

    dragDivider(500, -500);
    expect(screen.getByTestId('pane-state')).toHaveTextContent('1200');

    dragDivider(500, 1500);
    expect(screen.getByTestId('pane-state')).toHaveTextContent('collapsed');
  });
});
