import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { RecoverableLazyPanel } from './RecoverableLazyPanel';

function BrokenPanel(): never {
  throw new TypeError('Failed to fetch dynamically imported module');
}

describe('RecoverableLazyPanel', () => {
  afterEach(() => vi.restoreAllMocks());

  it('contains a nested failure without unmounting composer-owned state', () => {
    vi.spyOn(console, 'error').mockImplementation(() => undefined);

    render(
      <div>
        <div data-testid="attachments">design.png</div>
        <RecoverableLazyPanel>
          <BrokenPanel />
        </RecoverableLazyPanel>
      </div>,
    );

    expect(screen.getByTestId('attachments')).toHaveTextContent('design.png');
    expect(console.error).toHaveBeenCalledWith(
      '[Phoenix] A nested lazy panel failed to initialize.',
      expect.any(TypeError),
      expect.any(String),
    );
  });

  it('offers a non-destructive close transition for a replacement viewer', () => {
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
    const onClose = vi.fn();

    render(
      <RecoverableLazyPanel onClose={onClose}>
        <BrokenPanel />
      </RecoverableLazyPanel>,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Return to conversation' }));
    expect(onClose).toHaveBeenCalledOnce();
  });
});
