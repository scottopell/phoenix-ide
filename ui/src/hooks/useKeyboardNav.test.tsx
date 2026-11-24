// SIDE-03: Escape in context menu navigates away
//
// The global keyboard handler (useGlobalKeyboardShortcuts) navigates to /
// when Escape is pressed on a /c/ path. It does not check whether a context
// menu, modal, or popover is open. Pressing Escape while a context menu is
// open should close the menu WITHOUT triggering navigation.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { renderHook, act, fireEvent } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import type { ReactNode } from 'react';

const mockNavigate = vi.fn();
vi.mock('react-router-dom', async () => {
  const actual = await vi.importActual('react-router-dom');
  return {
    ...actual,
    useNavigate: () => mockNavigate,
    useLocation: () => ({ pathname: '/c/test-conversation' }),
  };
});

import { useGlobalKeyboardShortcuts, useKeyboardNav } from './useKeyboardNav';

function wrapper({ children }: { children: ReactNode }) {
  return <MemoryRouter initialEntries={['/c/test-conversation']}>{children}</MemoryRouter>;
}

describe('SIDE-03: global keyboard shortcut ownership', () => {
  let modal: HTMLDialogElement | null;
  beforeEach(() => {
    mockNavigate.mockClear();
    modal = null;
    const query = document.querySelector.bind(document);
    // jsdom has no native top layer; hosted Chromium covers actual show/showModal.
    vi.spyOn(document, 'querySelector').mockImplementation((selector: string) =>
      selector === 'dialog:modal' ? modal : query(selector));
  });
  afterEach(() => vi.restoreAllMocks());

  it('does not navigate to / on Escape when a popover/menu is open', () => {
    renderHook(() => useGlobalKeyboardShortcuts(), { wrapper });
    const menuEl = document.createElement('div');
    menuEl.className = 'conv-item-actions';
    menuEl.setAttribute('role', 'menu');
    document.body.appendChild(menuEl);

    act(() => {
      window.dispatchEvent(new KeyboardEvent('keydown', {
        key: 'Escape',
        bubbles: true,
        cancelable: true,
      }));
    });

    expect(mockNavigate).not.toHaveBeenCalled();
    menuEl.remove();
  });

  it('allows outside-dock help and composer Escape with an open nonmodal dialog', () => {
    renderHook(() => useGlobalKeyboardShortcuts(), { wrapper });
    const help = vi.fn();
    window.addEventListener('toggle-shortcut-help', help);
    const dialog = document.createElement('dialog');
    dialog.setAttribute('open', '');
    const transcript = document.createElement('div');
    transcript.tabIndex = 0;
    const composer = document.createElement('textarea');
    document.body.append(dialog, transcript, composer);
    transcript.focus();
    fireEvent.keyDown(transcript, { key: '?' });
    expect(help).toHaveBeenCalledOnce();
    composer.focus();
    fireEvent.keyDown(composer, { key: 'Escape' });
    expect(composer).not.toHaveFocus();
    expect(mockNavigate).not.toHaveBeenCalled();
    dialog.remove();
    transcript.remove();
    composer.remove();
    window.removeEventListener('toggle-shortcut-help', help);
  });

  it('suppresses global help and composer Escape while a native modal is open', () => {
    const dispatchSpy = vi.spyOn(window, 'dispatchEvent');
    renderHook(() => useGlobalKeyboardShortcuts(), { wrapper });
    const dialog = document.createElement('dialog');
    dialog.setAttribute('open', '');
    document.body.appendChild(dialog);
    modal = dialog;
    const composer = document.createElement('textarea');
    document.body.appendChild(composer);
    composer.focus();
    fireEvent.keyDown(composer, { key: 'Escape' });
    expect(composer).toHaveFocus();

    act(() => {
      window.dispatchEvent(new KeyboardEvent('keydown', { key: '?', bubbles: true }));
      window.dispatchEvent(new KeyboardEvent('keydown', { key: 'b', bubbles: true }));
    });

    expect(mockNavigate).not.toHaveBeenCalled();
    expect(dispatchSpy.mock.calls.some(([event]) => event.type === 'toggle-shortcut-help')).toBe(false);
    dialog.remove();
    composer.remove();
    dispatchSpy.mockRestore();
  });
});


describe('list keyboard navigation ownership', () => {
  it('does not intercept Enter from a focused button after selecting a row', () => {
    const onSelect = vi.fn();
    renderHook(() => useKeyboardNav({
      items: [{ id: 'row-1', slug: 'row-1' }],
      onSelect,
    }), { wrapper });
    const button = document.createElement('button');
    const onClick = vi.fn();
    button.addEventListener('click', onClick);
    document.body.appendChild(button);

    fireEvent.keyDown(window, { key: 'ArrowDown' });
    fireEvent.keyDown(button, { key: 'Enter' });
    fireEvent.click(button);

    expect(onClick).toHaveBeenCalledOnce();
    expect(onSelect).not.toHaveBeenCalled();
    button.remove();
  });
});
