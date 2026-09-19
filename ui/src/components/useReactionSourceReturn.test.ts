import { act, renderHook } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { useReactionSourceReturn } from './useReactionSourceReturn';
import type { ReactionSource } from '../conversation/InlineReactionStore';

const source: ReactionSource = { messageId: 'old-message', sequenceId: 1, occurrenceToken: 'old-row:old-message', quote: 'Older passage' };
function options() {
  return {
    scopeKey: 'conversation', historyKey: 'tail', loading: false, hasOlder: true,
    error: undefined as string | undefined,
    locate: vi.fn<(source: ReactionSource) => boolean>(() => false), loadOlder: vi.fn(() => new Promise<void>(() => {})),
  };
}

describe('return to retained paged reaction source', () => {
  it('loads successive older pages and positions the exact occurrence only when available', async () => {
    const initial = options();
    const { result, rerender } = renderHook(useReactionSourceReturn, { initialProps: initial });
    let returned!: Promise<boolean>;
    act(() => { returned = result.current(source, new AbortController().signal); });
    expect(initial.loadOlder).toHaveBeenCalledTimes(1);
    rerender({ ...initial, loading: true });
    rerender({ ...initial, historyKey: 'page-1' });
    expect(initial.loadOlder).toHaveBeenCalledTimes(2);
    rerender({ ...initial, historyKey: 'page-1', loading: true });
    const locate = vi.fn((candidate: ReactionSource) => candidate.occurrenceToken === 'old-row:old-message');
    rerender({ ...initial, historyKey: 'page-2', hasOlder: false, locate });
    expect(await returned).toBe(true);
    expect(locate).toHaveBeenCalledWith(source);
    expect(initial.loadOlder).toHaveBeenCalledTimes(2);
  });

  it('waits for an existing page request rather than issuing another', async () => {
    const initial = { ...options(), loading: true };
    const { result, rerender } = renderHook(useReactionSourceReturn, { initialProps: initial });
    let returned!: Promise<boolean>;
    act(() => { returned = result.current(source, new AbortController().signal); });
    expect(initial.loadOlder).not.toHaveBeenCalled();
    rerender({ ...initial, loading: false, hasOlder: false });
    expect(await returned).toBe(false);
  });

  it('retains a usable retry after a page error and stops at history exhaustion', async () => {
    const initial = options();
    let finishLoad!: () => void;
    initial.loadOlder.mockImplementation(() => new Promise<void>((resolve) => { finishLoad = resolve; }));
    const { result, rerender } = renderHook(useReactionSourceReturn, { initialProps: initial });
    let returned!: Promise<boolean>;
    act(() => { returned = result.current(source, new AbortController().signal); });
    rerender({ ...initial, error: 'offline' });
    await act(async () => { finishLoad(); });
    expect(await returned).toBe(false);
    act(() => { returned = result.current(source, new AbortController().signal); });
    expect(initial.loadOlder).toHaveBeenCalledTimes(2);
    rerender({ ...initial, loading: true });
    rerender({ ...initial, historyKey: 'last-page', hasOlder: false });
    expect(await returned).toBe(false);
  });

  it.each(['discard', 'navigate', 'unmount'] as const)('cancels on %s without later positioning or paging', async (reason) => {
    const initial = options();
    const { result, rerender, unmount } = renderHook(useReactionSourceReturn, { initialProps: initial });
    const controller = new AbortController();
    let returned!: Promise<boolean>;
    act(() => { returned = result.current(source, controller.signal); });
    rerender({ ...initial, loading: true });
    initial.locate.mockClear();
    if (reason === 'discard') act(() => controller.abort());
    if (reason === 'navigate') rerender({ ...initial, scopeKey: 'other-conversation', loading: true });
    if (reason === 'unmount') unmount();
    expect(await returned).toBe(false);
    if (reason !== 'unmount') rerender({ ...initial, historyKey: 'later-page' });
    expect(initial.locate).not.toHaveBeenCalled();
    expect(initial.loadOlder).toHaveBeenCalledTimes(1);
  });
});
