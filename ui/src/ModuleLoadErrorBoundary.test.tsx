import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { ModuleLoadErrorBoundary } from './ModuleLoadErrorBoundary';

function BrokenRoute(): never {
  throw new TypeError("Cannot read properties of undefined (reading 'ConversationPage')");
}

describe('ModuleLoadErrorBoundary', () => {
  afterEach(() => vi.restoreAllMocks());

  it('keeps a stable recovery surface when a lazy route rejects', () => {
    vi.spyOn(console, 'error').mockImplementation(() => undefined);

    render(
      <ModuleLoadErrorBoundary>
        <BrokenRoute />
      </ModuleLoadErrorBoundary>,
    );

    expect(screen.getByRole('alert')).toHaveTextContent('Part of Phoenix could not be loaded');
    expect(screen.getByText(/unsent attachments will be lost/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Reload Phoenix' })).toBeInTheDocument();
  });
});
