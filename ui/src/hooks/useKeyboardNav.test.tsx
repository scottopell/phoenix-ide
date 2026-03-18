// SIDE-03: Escape in context menu navigates away
//
// The global keyboard handler (useGlobalKeyboardShortcuts) navigates to /
// when Escape is pressed on a /c/ path. It does not check whether a context
// menu, modal, or popover is open. Pressing Escape while a context menu is
// open should close the menu WITHOUT triggering navigation.

import { describe, it, expect, vi } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import type { ReactNode } from 'react';

// We need to spy on navigate to detect if navigation happened
const mockNavigate = vi.fn();
vi.mock('react-router-dom', async () => {
  const actual = await vi.importActual('react-router-dom');
  return {
    ...actual,
    useNavigate: () => mockNavigate,
    useLocation: () => ({ pathname: '/c/test-conversation' }),
  };
});

import { useGlobalKeyboardShortcuts } from './useKeyboardNav';

function wrapper({ children }: { children: ReactNode }) {
  return <MemoryRouter initialEntries={['/c/test-conversation']}>{children}</MemoryRouter>;
}

describe('SIDE-03: Escape should not navigate when context menu is open', () => {
  beforeEach(() => {
    mockNavigate.mockClear();
  });

  it('does not navigate to / on Escape when a popover/menu is open', () => {
    renderHook(() => useGlobalKeyboardShortcuts(), { wrapper });

    // Simulate a context menu being open by adding a DOM element
    // that indicates an open menu (as ConversationList does with expandedId)
    const menuEl = document.createElement('div');
    menuEl.className = 'conv-item-actions';
    menuEl.setAttribute('role', 'menu');
    document.body.appendChild(menuEl);

    // Fire Escape key
    act(() => {
      const event = new KeyboardEvent('keydown', {
        key: 'Escape',
        bubbles: true,
        cancelable: true,
      });
      window.dispatchEvent(event);
    });

    // Navigation should NOT have happened because a menu was open.
    // The handler should check for open menus/popovers before navigating.
    expect(mockNavigate).not.toHaveBeenCalled();

    // Cleanup
    document.body.removeChild(menuEl);
  });
});
