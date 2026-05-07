/**
 * BrowserViewState surface contract (REQ-BT-018).
 *
 * The full slot-mutex resolution lives in ConversationPage's effects — those
 * are exercised by integration tests. This file pins the provider's surface
 * contract: open/close are independent, `browserSessionActive` propagates
 * from prop to context value, and `open` resets when scope changes.
 */
import { describe, it, expect, vi } from 'vitest';
import { act, render, renderHook } from '@testing-library/react';
import type { ReactNode } from 'react';
import {
  BrowserViewStateProvider,
  useBrowserViewState,
} from '../contexts/ViewerStateContext';

function wrapper({ children }: { children: ReactNode }) {
  return (
    <BrowserViewStateProvider browserSessionActive={false}>
      {children}
    </BrowserViewStateProvider>
  );
}

describe('BrowserViewState', () => {
  it('starts closed and inactive', () => {
    const { result } = renderHook(() => useBrowserViewState(), { wrapper });
    expect(result.current.open).toBe(false);
    expect(result.current.browserSessionActive).toBe(false);
  });

  it('open and close toggle independently of session state', () => {
    const { result } = renderHook(() => useBrowserViewState(), { wrapper });
    act(() => result.current.openPanel());
    expect(result.current.open).toBe(true);
    expect(result.current.browserSessionActive).toBe(false);
    act(() => result.current.closePanel());
    expect(result.current.open).toBe(false);
  });

  it('propagates browserSessionActive prop into context value', () => {
    let captured: ReturnType<typeof useBrowserViewState> | null = null;
    function Probe() {
      captured = useBrowserViewState();
      return null;
    }
    function Harness({ active }: { active: boolean }) {
      return (
        <BrowserViewStateProvider browserSessionActive={active}>
          <Probe />
        </BrowserViewStateProvider>
      );
    }
    const { rerender } = render(<Harness active={false} />);
    expect(captured!.browserSessionActive).toBe(false);

    rerender(<Harness active={true} />);
    expect(captured!.browserSessionActive).toBe(true);

    rerender(<Harness active={false} />);
    expect(captured!.browserSessionActive).toBe(false);
  });

  it('resets open when scopeKey changes', () => {
    let captured: ReturnType<typeof useBrowserViewState> | null = null;
    function Probe() {
      captured = useBrowserViewState();
      return null;
    }
    function Harness({ scope }: { scope: string }) {
      return (
        <BrowserViewStateProvider scopeKey={scope} browserSessionActive={false}>
          <Probe />
        </BrowserViewStateProvider>
      );
    }
    const { rerender } = render(<Harness scope="conv-a" />);
    act(() => {
      captured!.openPanel();
    });
    expect(captured!.open).toBe(true);

    // Switching scope simulates the user navigating to another conversation.
    // The provider drops `open` so the new scope never inherits the previous
    // conversation's panel-open state.
    rerender(<Harness scope="conv-b" />);
    expect(captured!.open).toBe(false);
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
