import { StrictMode, useLayoutEffect } from 'react';
import type { ReactNode } from 'react';
import { act, renderHook } from '@testing-library/react';
import type { Conversation } from '../api';
import { describe, expect, it, vi } from 'vitest';
import { ConversationContext } from '../conversation/ConversationContext';
import { ConversationStore } from '../conversation/ConversationStore';
import { DraftContext } from '../conversation/DraftContext';
import { DraftStore } from '../conversation/DraftStore';
import { DraftLifecycle, useDraftRecovery, useDraftValue } from './useDraft';

function wrapperFor(store: DraftStore) {
  return function Wrapper({ children }: { children: ReactNode }) {
    return <StrictMode><DraftContext.Provider value={store}>{children}</DraftContext.Provider></StrictMode>;
  };
}

function productDraftWrapper(drafts: DraftStore, conversations: ConversationStore) {
  const scope = 'product:pc-1';
  conversations.upsertSnapshot('transcript-1', {
    id: 'transcript-id',
    slug: 'transcript-1',
  } as Conversation);
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <ConversationContext.Provider value={conversations}>
        <DraftContext.Provider value={drafts}>
          <StrictMode>
            <DraftLifecycle slug={scope} productScope={{ legacyTranscriptSlug: 'transcript-1' }} />
            {children}
          </StrictMode>
        </DraftContext.Provider>
      </ConversationContext.Provider>
    );
  };
}

const payload = { id: 'one', text: 'raw', images: [], files: [], error: 'missing' };

describe('product composer draft lifecycle', () => {
  it.each(['setItem', 'removeItem'] as const)('hydrates readable legacy text even when migration %s throws', method => {
    localStorage.clear();
    localStorage.setItem('phoenix:draft:transcript-id', 'legacy draft');
    const warning = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const failure = vi.spyOn(localStorage, method).mockImplementation(() => { throw new Error('storage unavailable'); });
    const drafts = new DraftStore();
    const { result, unmount } = renderHook(() => useDraftValue('product:pc-1'), {
      wrapper: productDraftWrapper(drafts, new ConversationStore()),
    });
    try {
      expect(result.current).toBe('legacy draft');
      expect(localStorage.getItem('phoenix:draft:transcript-id')).toBe('legacy draft');
      if (method === 'removeItem') expect(localStorage.getItem('phoenix:draft:product:pc-1')).toBe('legacy draft');
      expect(warning).toHaveBeenCalled();
    } finally {
      unmount();
      failure.mockRestore();
      warning.mockRestore();
    }
  });

  it('migrates a transcript draft once and preserves an explicit clear over a surviving legacy key', () => {
    localStorage.clear();
    localStorage.setItem('phoenix:draft:transcript-id', 'legacy draft');
    const drafts = new DraftStore();
    const wrapper = productDraftWrapper(drafts, new ConversationStore());
    const { result, unmount } = renderHook(() => useDraftValue('product:pc-1'), { wrapper });

    expect(result.current).toBe('legacy draft');
    expect(localStorage.getItem('phoenix:draft:product:pc-1')).toBe('legacy draft');
    expect(localStorage.getItem('phoenix:draft:transcript-id')).toBeNull();

    // Reintroduce a surviving legacy key after the successful migration.
    localStorage.setItem('phoenix:draft:transcript-id', 'legacy draft');
    act(() => drafts.dispatch('product:pc-1', { type: 'clear_draft' }));
    unmount();

    const remounted = renderHook(() => useDraftValue('product:pc-1'), {
      wrapper: productDraftWrapper(drafts, new ConversationStore()),
    });
    expect(remounted.result.current).toBe('');
    expect(localStorage.getItem('phoenix:draft:product:pc-1')).toBe('');
  });
});

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
