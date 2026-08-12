import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { useRef, useState } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { SelectionDialog } from './SelectionDialog';

function DialogHarness({ dismissible = true }: { dismissible?: boolean }) {
  const [open, setOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  return (
    <>
      <button ref={triggerRef} type="button" onClick={() => setOpen(true)}>Open chooser</button>
      {open ? (
        <SelectionDialog
          title="Choose a value"
          description="One choice is required."
          onClose={() => setOpen(false)}
          dismissible={dismissible}
          restoreFocusRef={triggerRef}
        >
          <button type="button" data-selection-dialog-autofocus>First choice</button>
          <button type="button">Second choice</button>
        </SelectionDialog>
      ) : null}
    </>
  );
}

describe('SelectionDialog', () => {
  it('portals to the browser top layer, focuses content, and restores trigger focus on Escape', async () => {
    render(<DialogHarness />);
    const trigger = screen.getByRole('button', { name: 'Open chooser' });
    fireEvent.click(trigger);

    const dialog = screen.getByRole('dialog', { name: 'Choose a value' });
    expect(dialog.parentElement).toBe(document.body);
    expect(screen.getByRole('button', { name: 'First choice' })).toHaveFocus();

    fireEvent.keyDown(dialog, { key: 'Escape' });
    expect(screen.queryByRole('dialog', { name: 'Choose a value' })).not.toBeInTheDocument();
    await waitFor(() => expect(trigger).toHaveFocus());
  });

  it('dismisses from the dialog backdrop', async () => {
    render(<DialogHarness />);
    const trigger = screen.getByRole('button', { name: 'Open chooser' });
    fireEvent.click(trigger);
    const dialog = screen.getByRole('dialog', { name: 'Choose a value' });

    fireEvent.mouseDown(dialog);
    expect(screen.queryByRole('dialog', { name: 'Choose a value' })).not.toBeInTheDocument();
    await waitFor(() => expect(trigger).toHaveFocus());
  });

  it('blocks dismissal while its owner is pending', () => {
    render(<DialogHarness dismissible={false} />);
    fireEvent.click(screen.getByRole('button', { name: 'Open chooser' }));
    const dialog = screen.getByRole('dialog', { name: 'Choose a value' });

    fireEvent.keyDown(dialog, { key: 'Escape' });
    fireEvent.click(screen.getByRole('button', { name: 'Close Choose a value' }));
    fireEvent.mouseDown(dialog);

    expect(dialog).toBeInTheDocument();
  });

  it('does not throw when the invoking control disappears before teardown', () => {
    const error = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    const { unmount } = render(<DialogHarness />);
    fireEvent.click(screen.getByRole('button', { name: 'Open chooser' }));

    expect(() => unmount()).not.toThrow();
    expect(error).not.toHaveBeenCalled();
    error.mockRestore();
  });
});
