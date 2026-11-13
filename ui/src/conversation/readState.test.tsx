import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import { api } from '../api';
import { coordinatorUnread, useAutoMarkProductConversationRead } from './readState';

describe('useAutoMarkProductConversationRead', () => {
  let focused = true;
  beforeEach(() => {
    vi.useFakeTimers();
    focused = true;
    vi.spyOn(document, 'hasFocus').mockImplementation(() => focused);
    vi.spyOn(api, 'markProductConversationRead').mockResolvedValue({ state: 'read' });
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it('acknowledges on open, then only observes new output', async () => {
    const { rerender } = renderHook(
      ({ latest }) => useAutoMarkProductConversationRead('product', latest),
      { initialProps: { latest: 'm1' } },
    );
    expect(api.markProductConversationRead).toHaveBeenNthCalledWith(1, 'product', 'acknowledged');
    rerender({ latest: 'm2' });
    await act(async () => { vi.advanceTimersByTime(500); });
    expect(vi.mocked(api.markProductConversationRead).mock.calls.slice(1).every(([, kind]) => kind === 'observed')).toBe(true);
  });

  it('does not touch read state while the window lacks focus, and acknowledges on focus', async () => {
    focused = false;
    const { rerender } = renderHook(
      ({ latest }) => useAutoMarkProductConversationRead('product', latest),
      { initialProps: { latest: 'm1' } },
    );
    rerender({ latest: 'm2' });
    await act(async () => { vi.advanceTimersByTime(500); });
    expect(api.markProductConversationRead).not.toHaveBeenCalled();
    focused = true;
    act(() => { window.dispatchEvent(new Event('focus')); });
    expect(api.markProductConversationRead).toHaveBeenCalledWith('product', 'acknowledged');
  });

  it('sends one request on open and none for refocus without new output', async () => {
    renderHook(() => useAutoMarkProductConversationRead('product', 'm1'));
    await act(async () => { vi.advanceTimersByTime(500); });
    act(() => { window.dispatchEvent(new Event('focus')); });
    expect(api.markProductConversationRead).toHaveBeenCalledTimes(1);
    expect(api.markProductConversationRead).toHaveBeenCalledWith('product', 'acknowledged');
  });

  it('acknowledges again when a different conversation opens', () => {
    const { rerender } = renderHook(
      ({ id }) => useAutoMarkProductConversationRead(id, 'm1'),
      { initialProps: { id: 'a' } },
    );
    rerender({ id: 'b' });
    expect(api.markProductConversationRead).toHaveBeenCalledWith('a', 'acknowledged');
    expect(api.markProductConversationRead).toHaveBeenCalledWith('b', 'acknowledged');
  });
});

describe('coordinatorUnread', () => {
  it('maps the Global entry read state', () => {
    expect(coordinatorUnread({ coordinator: null })).toBeNull();
    expect(coordinatorUnread({ coordinator: { product_conversation_id: 'g', read_state: { state: 'read' } } })).toBeNull();
    expect(coordinatorUnread({ coordinator: { product_conversation_id: 'g', read_state: { state: 'unread', reason: 'marked' } } })).toBe('marked');
  });
});
