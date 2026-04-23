import { describe, it, expect, beforeEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import {
  useMessageQueue,
  derivePendingMessages,
  deriveFailedMessages,
  type QueuedMessage,
} from './useMessageQueue';

function queued(localId: string, overrides: Partial<QueuedMessage> = {}): QueuedMessage {
  return {
    localId,
    text: `text-${localId}`,
    images: [],
    timestamp: 0,
    status: 'pending',
    ...overrides,
  };
}

describe('derivePendingMessages', () => {
  it('filters out queue entries whose localId appears in server message ids', () => {
    const queue = [queued('a'), queued('b'), queued('c')];
    const out = derivePendingMessages(queue, ['b']);
    expect(out.map((q) => q.localId)).toEqual(['a', 'c']);
  });

  it('excludes failed messages — they render in the input area, not the list', () => {
    const queue = [queued('a'), queued('b', { status: 'failed' })];
    const out = derivePendingMessages(queue, []);
    expect(out.map((q) => q.localId)).toEqual(['a']);
  });

  it('returns an empty list when every queued entry has been echoed', () => {
    const queue = [queued('a'), queued('b')];
    const out = derivePendingMessages(queue, ['a', 'b']);
    expect(out).toEqual([]);
  });

  it('returns the full pending set when no server echoes yet', () => {
    const queue = [queued('a'), queued('b')];
    const out = derivePendingMessages(queue, []);
    expect(out.map((q) => q.localId)).toEqual(['a', 'b']);
  });

  // Acceptance criterion: "send a message, receive the SSE echo → rendered
  // exactly once (not twice during the overlap window)".
  it('acceptance: echoed message disappears from pending as soon as server has it', () => {
    const queue = [queued('msg-1')];
    // Pre-echo: in the pending list, one entry.
    expect(derivePendingMessages(queue, [])).toHaveLength(1);
    // Echo arrives: server now has msg-1. Pending collapses to zero, and the
    // consumer will render the row from atom.messages instead.
    expect(derivePendingMessages(queue, ['msg-1'])).toHaveLength(0);
  });

  // Acceptance criterion: "reload mid-send (message in queue, server has it)
  // → rendered once after rehydration".
  it('acceptance: rehydrated queue entry already echoed on server does not double-render', () => {
    const queue = [queued('msg-rehydrated')];
    const serverIds = ['msg-rehydrated'];
    expect(derivePendingMessages(queue, serverIds)).toEqual([]);
  });

  // Acceptance criterion: "reload mid-send (message in queue, server doesn't
  // have it) → renders as pending, resends on connection restored".
  it("acceptance: rehydrated queue entry not echoed by server stays pending", () => {
    const queue = [queued('msg-orphan')];
    expect(derivePendingMessages(queue, [])).toEqual([queue[0]]);
  });
});

describe('deriveFailedMessages', () => {
  it('returns only failed entries', () => {
    const queue = [
      queued('a'),
      queued('b', { status: 'failed' }),
      queued('c', { status: 'failed' }),
    ];
    const out = deriveFailedMessages(queue);
    expect(out.map((q) => q.localId)).toEqual(['b', 'c']);
  });

  it('returns [] when no failures', () => {
    expect(deriveFailedMessages([queued('a')])).toEqual([]);
  });
});

describe('useMessageQueue', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it('enqueue adds a pending message and returns it', () => {
    const { result } = renderHook(() => useMessageQueue('conv-1'));

    let msg: QueuedMessage | undefined;
    act(() => {
      msg = result.current.enqueue('hello', []);
    });

    expect(msg).toBeDefined();
    expect(msg!.text).toBe('hello');
    expect(msg!.status).toBe('pending');
    expect(result.current.queuedMessages).toHaveLength(1);
    expect(result.current.queuedMessages[0]!.localId).toBe(msg!.localId);
  });

  it('markFailed flips status to failed without removing the entry', () => {
    const { result } = renderHook(() => useMessageQueue('conv-1'));

    let msg: QueuedMessage | undefined;
    act(() => {
      msg = result.current.enqueue('fails', []);
    });
    act(() => {
      result.current.markFailed(msg!.localId);
    });

    expect(result.current.queuedMessages).toHaveLength(1);
    expect(result.current.queuedMessages[0]!.status).toBe('failed');
  });

  // Acceptance criterion: "send a message, POST fails → renders as failed,
  // retryable".
  it('acceptance: failed message is retryable — retry flips it back to pending', () => {
    const { result } = renderHook(() => useMessageQueue('conv-1'));

    let msg: QueuedMessage | undefined;
    act(() => {
      msg = result.current.enqueue('retry me', []);
      result.current.markFailed(msg!.localId);
    });
    expect(result.current.queuedMessages[0]!.status).toBe('failed');

    act(() => {
      result.current.retry(msg!.localId);
    });
    expect(result.current.queuedMessages[0]!.status).toBe('pending');
  });

  it('dismiss removes the entry', () => {
    const { result } = renderHook(() => useMessageQueue('conv-1'));

    let msg: QueuedMessage | undefined;
    act(() => {
      msg = result.current.enqueue('drop me', []);
      result.current.markFailed(msg!.localId);
      result.current.dismiss(msg!.localId);
    });

    expect(result.current.queuedMessages).toEqual([]);
  });

  it('does NOT expose markSent — the derivation replaces it', () => {
    const { result } = renderHook(() => useMessageQueue('conv-1'));
    // Runtime check: key absent on the hook's return value.
    expect('markSent' in (result.current as object)).toBe(false);
  });

  it('persists to localStorage and rehydrates on mount', () => {
    const { result: first } = renderHook(() => useMessageQueue('conv-1'));
    act(() => {
      first.current.enqueue('persist-me', []);
    });

    const { result: second } = renderHook(() => useMessageQueue('conv-1'));
    expect(second.current.queuedMessages).toHaveLength(1);
    expect(second.current.queuedMessages[0]!.text).toBe('persist-me');
    expect(second.current.queuedMessages[0]!.status).toBe('pending');
  });

  it("migrates legacy 'sending' status to 'pending' on rehydration", () => {
    // Seed storage with the pre-02676 shape where pending was stored as
    // 'sending'. The hook must coerce it so derivation works.
    const legacyEntry = {
      localId: 'legacy-1',
      text: 'old format',
      images: [],
      timestamp: 0,
      status: 'sending',
    };
    localStorage.setItem(
      'phoenix:queue:conv-legacy',
      JSON.stringify([legacyEntry]),
    );

    const { result } = renderHook(() => useMessageQueue('conv-legacy'));
    expect(result.current.queuedMessages).toHaveLength(1);
    expect(result.current.queuedMessages[0]!.status).toBe('pending');
  });
});
