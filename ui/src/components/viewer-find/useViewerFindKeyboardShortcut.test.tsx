import { render } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { FocusScopeProvider, useRegisterFocusScope } from '../../hooks/useFocusScope';
import { useViewerFindKeyboardShortcut } from './useViewerFindKeyboardShortcut';

function ShortcutHarness({ scopeId, onOpen }: { scopeId: string; onOpen: () => void }) {
  useRegisterFocusScope(scopeId);
  useViewerFindKeyboardShortcut({ scopeId, onOpen });
  return <input aria-label="editor" />;
}

function InactiveScopeHarness({ scopeId, onOpen }: { scopeId: string; onOpen: () => void }) {
  useRegisterFocusScope('different-scope');
  useViewerFindKeyboardShortcut({ scopeId, onOpen });
  return <div>inactive</div>;
}

describe('useViewerFindKeyboardShortcut', () => {
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
