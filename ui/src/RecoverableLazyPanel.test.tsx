import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { RecoverableLazyPanel } from './RecoverableLazyPanel';
import { ModuleLoadErrorBoundary } from './ModuleLoadErrorBoundary';
import { clearModuleAcquisitionFailure, recordModuleAcquisitionFailure } from './moduleAcquisitionFailure';

function BrokenPanel({ message = 'Failed to fetch dynamically imported module' }: { message?: string }): never {
  throw new TypeError(message);
}

describe('RecoverableLazyPanel', () => {
  afterEach(() => {
    clearModuleAcquisitionFailure();
    vi.restoreAllMocks();
  });

  it('contains a nested module failure without unmounting composer-owned state', () => {
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
    recordModuleAcquisitionFailure();

    render(
      <div>
        <div data-testid="attachments">design.png</div>
        <RecoverableLazyPanel>
          <BrokenPanel />
        </RecoverableLazyPanel>
      </div>,
    );

    expect(screen.getByTestId('attachments')).toHaveTextContent('design.png');
    expect(screen.getByRole('alert')).toHaveTextContent('This feature could not be loaded');
  });

  it('offers a non-destructive close transition for a replacement viewer', () => {
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
    const onClose = vi.fn();
    recordModuleAcquisitionFailure();

    render(
      <RecoverableLazyPanel onClose={onClose}>
        <BrokenPanel />
      </RecoverableLazyPanel>,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Return to conversation' }));
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('lets ordinary component failures reach the visible root fallback', () => {
    vi.spyOn(console, 'error').mockImplementation(() => undefined);

    render(
      <ModuleLoadErrorBoundary>
        <RecoverableLazyPanel>
          <BrokenPanel message="ordinary render failure" />
        </RecoverableLazyPanel>
      </ModuleLoadErrorBoundary>,
    );

    expect(screen.getByText('Part of Phoenix could not be loaded')).toBeInTheDocument();
  });
});
