import { describe, expect, it, beforeEach } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { useResizablePane } from './useResizablePane';

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
