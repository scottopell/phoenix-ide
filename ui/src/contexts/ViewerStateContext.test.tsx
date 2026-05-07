/**
 * BrowserViewState mutex semantics (REQ-BT-018).
 *
 * The full slot-mutex resolution lives in ConversationPage's effects — those
 * are exercised by integration tests. This file just pins the provider's
 * surface contract: open/close are independent, hasActivated is sticky
 * across a close, and markActivated is idempotent.
 */
import { describe, it, expect, vi } from 'vitest';
import { act, render, renderHook } from '@testing-library/react';
import type { ReactNode } from 'react';
import {
  BrowserViewStateProvider,
  useBrowserViewState,
} from '../contexts/ViewerStateContext';

function wrapper({ children }: { children: ReactNode }) {
  return <BrowserViewStateProvider>{children}</BrowserViewStateProvider>;
}

describe('BrowserViewState', () => {
  it('starts closed and not activated', () => {
    const { result } = renderHook(() => useBrowserViewState(), { wrapper });
    expect(result.current.open).toBe(false);
    expect(result.current.hasActivated).toBe(false);
  });

  it('open and close toggle independently of activation', () => {
    const { result } = renderHook(() => useBrowserViewState(), { wrapper });
    act(() => result.current.openPanel());
    expect(result.current.open).toBe(true);
    expect(result.current.hasActivated).toBe(false);
    act(() => result.current.closePanel());
    expect(result.current.open).toBe(false);
  });

  it('hasActivated is sticky across close', () => {
    const { result } = renderHook(() => useBrowserViewState(), { wrapper });
    act(() => {
      result.current.markActivated();
      result.current.openPanel();
    });
    expect(result.current.hasActivated).toBe(true);
    act(() => result.current.closePanel());
    // The whole point of the sticky flag: closing the panel doesn't
    // un-activate; the user can re-open from the manual affordance later.
    expect(result.current.hasActivated).toBe(true);
    expect(result.current.open).toBe(false);
  });

  it('markActivated is idempotent', () => {
    const { result } = renderHook(() => useBrowserViewState(), { wrapper });
    act(() => result.current.markActivated());
    const first = result.current.markActivated;
    act(() => result.current.markActivated());
    // Identity stability matters because effects key off these refs.
    expect(result.current.markActivated).toBe(first);
    expect(result.current.hasActivated).toBe(true);
  });

  it('resets open and hasActivated when scopeKey changes', () => {
    // Component-style test: a parent owns the scope and we drive a change
    // via React state. renderHook's wrapper is fixed at mount, so a
    // straightforward render+rerender of a tiny harness is the cleanest path.
    let captured: ReturnType<typeof useBrowserViewState> | null = null;
    function Probe() {
      captured = useBrowserViewState();
      return null;
    }
    function Harness({ scope }: { scope: string }) {
      return (
        <BrowserViewStateProvider scopeKey={scope}>
          <Probe />
        </BrowserViewStateProvider>
      );
    }
    const { rerender } = render(<Harness scope="conv-a" />);
    act(() => {
      captured!.markActivated();
      captured!.openPanel();
    });
    expect(captured!.open).toBe(true);
    expect(captured!.hasActivated).toBe(true);

    // Switching scope simulates the user navigating to another conversation.
    // The provider must drop both open and hasActivated so the new scope
    // never inherits the previous one's panel state.
    rerender(<Harness scope="conv-b" />);
    expect(captured!.open).toBe(false);
    expect(captured!.hasActivated).toBe(false);
  });

  it('throws when used outside the provider', () => {
    // Suppress the React-thrown error log so the test output is quiet.
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {});
    expect(() => renderHook(() => useBrowserViewState())).toThrow(
      /must be used inside <BrowserViewStateProvider>/,
    );
    spy.mockRestore();
  });
});
