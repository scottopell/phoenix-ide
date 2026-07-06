import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { useState } from 'react';
import { RenameDialog } from './RenameDialog';

describe('RenameDialog', () => {
  it('does not render the AI generation button without an onGenerate handler', () => {
    render(
      <RenameDialog
        visible
        currentName="current-slug"
        onRename={vi.fn()}
        onCancel={vi.fn()}
        error={undefined}
      />,
    );

    expect(screen.queryByRole('button', { name: /generate with ai/i })).not.toBeInTheDocument();
  });

  it('prevents dismissing while generation is in flight', async () => {
    const onCancel = vi.fn();
    render(
      <RenameDialog
        visible
        currentName="current-slug"
        onRename={vi.fn()}
        onGenerate={() => new Promise<void>(() => {})}
        onCancel={onCancel}
        error={undefined}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: /generate with ai/i }));
    fireEvent.keyDown(document, { key: 'Escape' });
    fireEvent.click(screen.getByText('Rename Conversation').closest('.modal-overlay')!);
    fireEvent.click(screen.getByRole('button', { name: /cancel/i }));

    expect(screen.getByRole('button', { name: /cancel/i })).toBeDisabled();
    expect(onCancel).not.toHaveBeenCalled();
  });

  it('renders the AI generation button and shows in-flight state', async () => {
    let resolveGenerate!: () => void;
    const onGenerate = vi.fn(
      () => new Promise<void>((resolve) => {
        resolveGenerate = resolve;
      }),
    );

    render(
      <RenameDialog
        visible
        currentName="current-slug"
        onRename={vi.fn()}
        onGenerate={onGenerate}
        onCancel={vi.fn()}
        error={undefined}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: /generate with ai/i }));

    expect(onGenerate).toHaveBeenCalledOnce();
    expect(screen.getByRole('button', { name: /generating/i })).toBeDisabled();
    expect(screen.getByRole('button', { name: /rename/i })).toBeDisabled();
    expect(screen.getByRole('textbox')).toBeDisabled();

    resolveGenerate();
    await waitFor(() => {
      expect(screen.queryByRole('button', { name: /generating/i })).not.toBeInTheDocument();
    });
  });

  it('surfaces generation failure through the existing error area without closing', async () => {
    function Harness() {
      const [error, setError] = useState<string | undefined>();
      return (
        <RenameDialog
          visible
          currentName="current-slug"
          onRename={vi.fn()}
          onGenerate={async () => {
            setError('Failed to generate name');
            throw new Error('Failed to generate name');
          }}
          onCancel={vi.fn()}
          error={error}
        />
      );
    }

    render(<Harness />);
    fireEvent.click(screen.getByRole('button', { name: /generate with ai/i }));

    expect(await screen.findByText('Failed to generate name')).toBeInTheDocument();
    expect(screen.getByRole('dialog', { hidden: true })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /generate with ai/i })).not.toBeDisabled();
  });
});
