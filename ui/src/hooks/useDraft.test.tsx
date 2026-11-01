import { StrictMode, useLayoutEffect } from 'react';
import type { ReactNode } from 'react';
import { act, renderHook } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { DraftContext } from '../conversation/DraftContext';
import { DraftStore } from '../conversation/DraftStore';
import { useDraftRecovery } from './useDraft';

function wrapperFor(store: DraftStore) {
  return function Wrapper({ children }: { children: ReactNode }) {
    return <StrictMode><DraftContext.Provider value={store}>{children}</DraftContext.Provider></StrictMode>;
  };
}

const payload = { id: 'one', text: 'raw', images: [], files: [], error: 'missing' };

describe('useDraftRecovery lifetime', () => {
  it('reacquires on same-slug removal even when rejected submissions stay empty', () => {
    const store = new DraftStore();
    const { result } = renderHook(() => useDraftRecovery('a'), { wrapper: wrapperFor(store) });
    const old = result.current;
    act(() => store.remove('a'));
    expect(result.current).not.toBe(old);
    act(() => result.current.retainRejected(payload));
    expect(result.current.rejected).toEqual([payload]);
    act(() => old.removeRejected('one'));
    expect(result.current.rejected).toEqual([payload]);
  });

  it('retains the lifetime across ordinary unmount and remount', () => {
    const store = new DraftStore();
    const wrapper = wrapperFor(store);
    const first = renderHook(() => useDraftRecovery('a'), { wrapper });
    const recovery = first.result.current;
    first.unmount();
    act(() => recovery.retainRejected(payload));
    const second = renderHook(() => useDraftRecovery('a'), { wrapper });
    expect(second.result.current.retainRejected).toBe(recovery.retainRejected);
    expect(second.result.current.rejected).toEqual([payload]);
  });

  it('detects removal between render and subscription without waiting for an effect or another mutation', () => {
    const store = new DraftStore();
    const old = store.recovery('a');
    const wrapper = ({ children }: { children: ReactNode }) => <DraftContext.Provider value={store}>{children}</DraftContext.Provider>;
    const { result } = renderHook(() => {
      const recovery = useDraftRecovery('a');
      useLayoutEffect(() => store.remove('a'), []);
      return recovery;
    }, { wrapper });
    expect(result.current.retainRejected).not.toBe(old.retainRejected);
    act(() => result.current.retainRejected(payload));
    expect(result.current.rejected).toEqual([payload]);
  });
});
