import { describe, it, expect, beforeEach } from 'vitest';
import { renderHook, act, render } from '@testing-library/react';
import { useState } from 'react';
import {
  useMessageQueue,
  derivePendingMessages,
  deriveFailedMessages,
  type QueuedMessage,
} from './useMessageQueue';

function queued(localId: string, overrides: Partial<QueuedMessage> = {}): QueuedMessage {
  return {
    localId,
    conversationId: 'conv-1',
    text: `text-${localId}`,
    images: [],
    timestamp: 0,
    status: 'pending',
    ...overrides,
  };
}

// Write entries into a conversation's storage key. Each entry is tagged with
// that conversation by default; pass an explicit `conversationId` to write a
// foreign-tagged row (the contamination cases).
function seed(
  conversationId: string,
  ...entries: Array<{ localId: string } & Partial<QueuedMessage>>
): void {
  localStorage.setItem(
    `phoenix:queue:${conversationId}`,
    JSON.stringify(entries.map((e) => queued(e.localId, { conversationId, ...e }))),
  );
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

  // Regression: when the ConversationPage instance is reused across a
//   `/c/:fooSlug` → `/c/:barSlug` navigation, the hook must reload (or
  // clear) the queue. Before the fix, an `initializedRef` guard skipped
  // the reload on truthy→truthy conversationId transitions, so conv A's
  // queued items rendered as pending in conv B's view.
  it('reloads the queue when conversationId changes between two truthy values', () => {
    // Seed conv-a and conv-b storage with disjoint queues.
    seed('conv-a', { localId: 'a-1', text: 'from A' });
    seed('conv-b', { localId: 'b-1', text: 'from B' });

    const { result, rerender } = renderHook(
      ({ id }: { id: string }) => useMessageQueue(id),
      { initialProps: { id: 'conv-a' } },
    );
    expect(result.current.queuedMessages.map((q) => q.text)).toEqual(['from A']);

    rerender({ id: 'conv-b' });
    expect(result.current.queuedMessages.map((q) => q.text)).toEqual(['from B']);
  });

  it('clears the queue when conversationId becomes undefined', () => {
    seed('conv-a', { localId: 'a-1' });
    const { result, rerender } = renderHook(
      ({ id }: { id: string | undefined }) => useMessageQueue(id),
      { initialProps: { id: 'conv-a' as string | undefined } },
    );
    expect(result.current.queuedMessages).toHaveLength(1);

    rerender({ id: undefined });
    expect(result.current.queuedMessages).toEqual([]);
  });

  // Regression (cross-conversation queue bleed): a write that raced an
  // A↔B switch persisted conversation B's pending entries under
  // conversation A's storage key. Tagged with B's id, they could never
  // reconcile against A's `atom.messages` (their localIds match B's server
  // rows, not A's) and rendered as phantom `pending` bubbles that survived
  // reload and restart. The load-path contamination guard drops them.
  it('drops foreign-conversation entries stamped under the wrong key (self-heal)', () => {
    // conv-a's storage was contaminated with a conv-b-tagged entry plus a
    // legitimate conv-a entry.
    seed(
      'conv-a',
      { localId: 'foreign', text: 'belongs to B', conversationId: 'conv-b' },
      { localId: 'native', text: 'belongs to A' },
    );

    const { result } = renderHook(() => useMessageQueue('conv-a'));

    expect(result.current.queuedMessages.map((q) => q.text)).toEqual(['belongs to A']);
  });

  it('rewrites storage without the foreign entry on the next mutation', () => {
    seed(
      'conv-a',
      { localId: 'foreign', text: 'belongs to B', conversationId: 'conv-b' },
      { localId: 'native', text: 'belongs to A' },
    );

    const { result } = renderHook(() => useMessageQueue('conv-a'));
    act(() => {
      result.current.enqueue('fresh A message', []);
    });

    const persisted = JSON.parse(
      localStorage.getItem('phoenix:queue:conv-a')!,
    ) as QueuedMessage[];
    expect(persisted.map((q) => q.localId)).toEqual(['native', expect.any(String)]);
    expect(persisted.every((q) => q.conversationId === 'conv-a')).toBe(true);
  });

  // Regression: a mutation bound to conversation A must never fold a stale
  // localId from conversation B into A's storage. The write path reads the
  // current key from localStorage rather than a shared React `prev`, so a
  // foreign localId is simply absent and the mutation no-ops out.
  it('marking a foreign localId failed does not resurrect it into the current queue', () => {
    seed('conv-a', { localId: 'native', text: 'belongs to A' });

    const { result } = renderHook(() => useMessageQueue('conv-a'));
    act(() => {
      // 'b-orphan' belongs to conversation B and was never enqueued under A.
      result.current.markFailed('b-orphan');
    });

    expect(result.current.queuedMessages.map((q) => q.localId)).toEqual(['native']);
    const persisted = JSON.parse(
      localStorage.getItem('phoenix:queue:conv-a')!,
    ) as QueuedMessage[];
    expect(persisted.map((q) => q.localId)).toEqual(['native']);
  });

  // Regression: a callback captured under conversation A and fired after
  // navigating to B (e.g. a POST that resolves post-navigation) must persist
  // to A's own key without pushing A's queue into B's rendered view.
  it('a stale mutation from a prior conversation does not pollute the current view', () => {
    const { result, rerender } = renderHook(
      ({ id }: { id: string }) => useMessageQueue(id),
      { initialProps: { id: 'conv-a' } },
    );

    // Capture A's enqueue, then navigate to B (empty queue).
    const enqueueA = result.current.enqueue;
    rerender({ id: 'conv-b' });
    expect(result.current.queuedMessages).toEqual([]);

    // The stale A-bound callback resolves now.
    act(() => {
      enqueueA('late A message', []);
    });

    // B's view is untouched...
    expect(result.current.queuedMessages).toEqual([]);
    // ...but A's stored queue did receive the message.
    const aStored = JSON.parse(
      localStorage.getItem('phoenix:queue:conv-a')!,
    ) as QueuedMessage[];
    expect(aStored.map((q) => q.text)).toEqual(['late A message']);
  });

  it('stamps newly enqueued messages with the active conversationId', () => {
    const { result } = renderHook(() => useMessageQueue('conv-active'));
    let msg: QueuedMessage | undefined;
    act(() => {
      msg = result.current.enqueue('tagged', []);
    });
    expect(msg!.conversationId).toBe('conv-active');
    expect(result.current.queuedMessages[0]!.conversationId).toBe('conv-active');
  });

  // Self-heal of pre-existing contamination: entries written before the
  // conversation tag existed carry no `conversationId`, so their ownership
  // cannot be confirmed. They are exactly the rows a pre-tag write race could
  // have stranded under another conversation's key, so rehydration drops them
  // rather than blessing them as belonging here (which would keep the phantom
  // pending bubble alive across reload).
  it('drops untagged legacy entries whose ownership cannot be confirmed', () => {
    const legacyEntry = {
      localId: 'legacy-1',
      text: 'old format',
      images: [],
      timestamp: 0,
      status: 'pending',
    };
    localStorage.setItem(
      'phoenix:queue:conv-legacy',
      JSON.stringify([legacyEntry]),
    );

    const { result } = renderHook(() => useMessageQueue('conv-legacy'));
    expect(result.current.queuedMessages).toEqual([]);
  });
});

// Regression: visit A → visit B → return to A. Under the prior `useEffect`-
// based reset the *render* under conv-A's id (after returning) briefly saw
// conv-B's queue, then committed conv-A's. The bug was visually invisible on
// first visits because there was nothing to flash from. Asserting on the
// rendered DOM after each switch — without an intervening commit cycle that
// would mask a re-derivation bug — exercises the in-render reset.
describe('useMessageQueue — returning navigation does not flash stale queue', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  function Probe({ id }: { id: string | undefined }) {
    const { queuedMessages } = useMessageQueue(id);
    return (
      <ul data-testid="queue">
        {queuedMessages.map((m) => (
          <li key={m.localId} data-localid={m.localId}>{m.text}</li>
        ))}
      </ul>
    );
  }

  function Harness({ initial }: { initial: string | undefined }) {
    const [id, setId] = useState<string | undefined>(initial);
    return (
      <>
        <button data-testid="to-a" onClick={() => setId('conv-a')}>A</button>
        <button data-testid="to-b" onClick={() => setId('conv-b')}>B</button>
        <Probe id={id} />
      </>
    );
  }

  it('renders B-only items immediately after A→B switch (no A flash)', () => {
    seed('conv-a', { localId: 'a-1', text: 'from A' });
    seed('conv-b', { localId: 'b-1', text: 'from B' });

    const { getByTestId, queryByText } = render(<Harness initial="conv-a" />);
    expect(queryByText('from A')).not.toBeNull();

    act(() => {
      getByTestId('to-b').click();
    });

    const items = getByTestId('queue').querySelectorAll('li');
    expect(items).toHaveLength(1);
    expect(items[0]!.textContent).toBe('from B');
    expect(queryByText('from A')).toBeNull();
  });

  it('renders A-only items immediately on returning A→B→A (no B flash)', () => {
    seed('conv-a', { localId: 'a-1', text: 'from A' });
    seed('conv-b', { localId: 'b-1', text: 'from B' });

    const { getByTestId, queryByText } = render(<Harness initial="conv-a" />);
    act(() => {
      getByTestId('to-b').click();
    });
    act(() => {
      getByTestId('to-a').click();
    });

    const items = getByTestId('queue').querySelectorAll('li');
    expect(items).toHaveLength(1);
    expect(items[0]!.textContent).toBe('from A');
    expect(queryByText('from B')).toBeNull();
  });

  it('renders empty list immediately when conversationId becomes undefined', () => {
    seed('conv-a', { localId: 'a-1', text: 'from A' });

    function ClearHarness() {
      const [id, setId] = useState<string | undefined>('conv-a');
      return (
        <>
          <button data-testid="clear" onClick={() => setId(undefined)}>clear</button>
          <Probe id={id} />
        </>
      );
    }

    const { getByTestId, queryByText } = render(<ClearHarness />);
    expect(queryByText('from A')).not.toBeNull();

    act(() => {
      getByTestId('clear').click();
    });

    expect(getByTestId('queue').querySelectorAll('li')).toHaveLength(0);
  });
});
