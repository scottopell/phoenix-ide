import { render, screen, fireEvent } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import {
  FocusScopeProvider,
  useKeyboardRouterShortcut,
  useRegisterFocusScope,
} from '../../hooks/useFocusScope';
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

function RouterHarness({
  id,
  scopeId,
  layer,
  keyName,
  onHandle,
  enabled = true,
  allowWhenNoActiveScope = false,
  dialogOpen = false,
}: {
  id: string;
  scopeId?: string;
  layer: 'modal' | 'viewer' | 'passive-content';
  keyName: 'mod+f' | 'Escape';
  onHandle: () => void;
  enabled?: boolean;
  allowWhenNoActiveScope?: boolean;
  dialogOpen?: boolean;
}) {
  useRegisterFocusScope(scopeId ?? null);
  useKeyboardRouterShortcut({
    id,
    layer,
    key: keyName,
    ...(scopeId ? { scopeId } : {}),
    enabled,
    allowWhenNoActiveScope,
    dialogOpen,
    handler: (event) => {
      event.preventDefault();
      onHandle();
    },
  });
  return <button type="button">{id}</button>;
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

  it('routes repeated find from the find input back to the owning session', () => {
    const onOpen = vi.fn();
    render(
      <FocusScopeProvider>
        <ShortcutHarness scopeId="viewer" onOpen={onOpen} />
      </FocusScopeProvider>,
    );

    const findInput = screen.getByRole('textbox', { name: 'find' });
    fireEvent.keyDown(findInput, { key: 'f', metaKey: true, bubbles: true, cancelable: true });
    expect(onOpen).toHaveBeenCalledTimes(1);
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

describe('keyboard router', () => {
  it('routes viewer shortcuts ahead of passive-content shortcuts', () => {
    const viewer = vi.fn();
    const passive = vi.fn();
    render(
      <FocusScopeProvider>
        <RouterHarness id="passive" scopeId="surface" layer="passive-content" keyName="mod+f" onHandle={passive} />
        <RouterHarness id="viewer" scopeId="surface" layer="viewer" keyName="mod+f" onHandle={viewer} />
      </FocusScopeProvider>,
    );

    fireEvent.keyDown(window, { key: 'f', metaKey: true, bubbles: true, cancelable: true });
    expect(viewer).toHaveBeenCalledTimes(1);
    expect(passive).not.toHaveBeenCalled();
  });

  it('lets modal shortcuts block viewer shortcuts', () => {
    const modal = vi.fn();
    const viewer = vi.fn();
    render(
      <FocusScopeProvider>
        <RouterHarness id="viewer" scopeId="surface" layer="viewer" keyName="Escape" onHandle={viewer} />
        <RouterHarness id="modal" scopeId="surface" layer="modal" keyName="Escape" onHandle={modal} />
      </FocusScopeProvider>,
    );

    fireEvent.keyDown(window, { key: 'Escape', bubbles: true, cancelable: true });
    expect(modal).toHaveBeenCalledTimes(1);
    expect(viewer).not.toHaveBeenCalled();
  });

  it('uses focus-scope ownership to choose between inline viewers', () => {
    const first = vi.fn();
    const second = vi.fn();
    render(
      <FocusScopeProvider>
        <RouterHarness id="first" scopeId="viewer-a" layer="viewer" keyName="mod+f" onHandle={first} />
        <RouterHarness id="second" scopeId="viewer-b" layer="viewer" keyName="mod+f" onHandle={second} />
      </FocusScopeProvider>,
    );

    fireEvent.keyDown(window, { key: 'f', ctrlKey: true, bubbles: true, cancelable: true });
    expect(second).toHaveBeenCalledTimes(1);
    expect(first).not.toHaveBeenCalled();
  });

  it('cleans up registrations on unmount', () => {
    const onHandle = vi.fn();
    const { unmount } = render(
      <FocusScopeProvider>
        <RouterHarness id="viewer" scopeId="surface" layer="viewer" keyName="mod+f" onHandle={onHandle} />
      </FocusScopeProvider>,
    );

    unmount();
    fireEvent.keyDown(window, { key: 'f', metaKey: true, bubbles: true, cancelable: true });
    expect(onHandle).not.toHaveBeenCalled();
  });

  it('installs only one global listener per provider', () => {
    const add = vi.spyOn(window, 'addEventListener');
    const remove = vi.spyOn(window, 'removeEventListener');
    const onHandle = vi.fn();
    const beforeAdds = add.mock.calls.filter(([type]) => type === 'keydown').length;
    const beforeRemoves = remove.mock.calls.filter(([type]) => type === 'keydown').length;
    const { rerender, unmount } = render(
      <FocusScopeProvider>
        <RouterHarness id="one" scopeId="surface" layer="viewer" keyName="mod+f" onHandle={onHandle} />
      </FocusScopeProvider>,
    );

    rerender(
      <FocusScopeProvider>
        <>
          <RouterHarness id="one" scopeId="surface" layer="viewer" keyName="mod+f" onHandle={onHandle} />
          <RouterHarness id="two" scopeId="surface" layer="passive-content" keyName="Escape" onHandle={onHandle} />
        </>
      </FocusScopeProvider>,
    );

    const afterAdds = add.mock.calls.filter(([type]) => type === 'keydown').length;
    expect(afterAdds - beforeAdds).toBe(1);
    unmount();
    const afterRemoves = remove.mock.calls.filter(([type]) => type === 'keydown').length;
    expect(afterRemoves - beforeRemoves).toBe(1);
  });
});
