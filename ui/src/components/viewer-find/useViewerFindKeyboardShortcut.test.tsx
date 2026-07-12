import { render, screen, fireEvent } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { FocusScopeProvider, useRegisterFocusScope } from '../../hooks/useFocusScope';
import { useViewerFindKeyboardShortcut } from './useViewerFindKeyboardShortcut';

function ShortcutHarness({
  scopeId,
  onOpen,
  dialogOpen = false,
}: { scopeId: string; onOpen: () => void; dialogOpen?: boolean }) {
  useRegisterFocusScope(scopeId);
  useViewerFindKeyboardShortcut({ scopeId, onOpen, dialogOpen });
  return (
    <>
      <input aria-label="editor" />
      <button type="button">Dialog button</button>
      <input aria-label="find" data-viewer-find-input="true" />
    </>
  );
}

function InactiveScopeHarness({ scopeId, onOpen }: { scopeId: string; onOpen: () => void }) {
  useRegisterFocusScope('different-scope');
  useViewerFindKeyboardShortcut({ scopeId, onOpen });
  return <div>inactive</div>;
}

function EmptyScopeHarness({ onOpen }: { onOpen: () => void }) {
  useViewerFindKeyboardShortcut({ scopeId: 'transcript', onOpen, allowWhenNoActiveScope: true });
  return <div>transcript</div>;
}

describe('useViewerFindKeyboardShortcut', () => {
  it('can claim the shortcut without permanently occupying the scope stack', () => {
    const onOpen = vi.fn();
    render(<FocusScopeProvider><EmptyScopeHarness onOpen={onOpen} /></FocusScopeProvider>);

    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'f', metaKey: true, bubbles: true, cancelable: true }));
    expect(onOpen).toHaveBeenCalledTimes(1);
  });

  it('opens only for the active focus scope', () => {
    const onOpen = vi.fn();
    render(
      <FocusScopeProvider>
        <ShortcutHarness scopeId="viewer" onOpen={onOpen} />
      </FocusScopeProvider>,
    );

    const event = new KeyboardEvent('keydown', { key: 'f', ctrlKey: true, bubbles: true, cancelable: true });
    const preventDefault = vi.spyOn(event, 'preventDefault');
    window.dispatchEvent(event);

    expect(preventDefault).toHaveBeenCalled();
    expect(onOpen).toHaveBeenCalledTimes(1);
  });

  it('does not reopen find when the find input itself handles the repeated shortcut', () => {
    const onOpen = vi.fn();
    render(
      <FocusScopeProvider>
        <ShortcutHarness scopeId="viewer" onOpen={onOpen} />
      </FocusScopeProvider>,
    );

    const findInput = screen.getByRole('textbox', { name: 'find' });
    fireEvent.keyDown(findInput, { key: 'f', metaKey: true, bubbles: true, cancelable: true });
    expect(onOpen).not.toHaveBeenCalled();
  });

  it('stands down while a dialog in the same surface is open', () => {
    const onOpen = vi.fn();
    render(
      <FocusScopeProvider>
        <ShortcutHarness scopeId="viewer" onOpen={onOpen} dialogOpen />
      </FocusScopeProvider>,
    );

    const dialogButton = screen.getByRole('button', { name: 'Dialog button' });
    fireEvent.keyDown(dialogButton, { key: 'f', ctrlKey: true, bubbles: true, cancelable: true });
    expect(onOpen).not.toHaveBeenCalled();
  });

  it('ignores Cmd/Ctrl+F when another scope is active or when typing in an input', () => {
    const onOpen = vi.fn();
    const { rerender, getByRole } = render(
      <FocusScopeProvider>
        <InactiveScopeHarness scopeId="viewer" onOpen={onOpen} />
      </FocusScopeProvider>,
    );

    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'f', metaKey: true, bubbles: true, cancelable: true }));
    expect(onOpen).not.toHaveBeenCalled();

    rerender(
      <FocusScopeProvider>
        <ShortcutHarness scopeId="viewer" onOpen={onOpen} />
      </FocusScopeProvider>,
    );

    const input = getByRole('textbox', { name: 'editor' });
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'f', ctrlKey: true, bubbles: true, cancelable: true }));
    expect(onOpen).not.toHaveBeenCalled();
  });
});
