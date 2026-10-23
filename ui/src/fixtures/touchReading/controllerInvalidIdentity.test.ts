import { afterEach, describe, expect, it, vi } from 'vitest';
import { ConversationStore } from '../../conversation/ConversationStore';
import { createInitialAtom, type ConversationAtom } from '../../conversation/atom';
import type { Conversation } from '../../api';
import { observeConsumption, type ConsumptionReceipt } from './observeConsumption';

// Proposed additional isolated test file; controlled malformed snapshots only.
// This is not evidence of a production reducer admitting malformed identities.
afterEach(() => vi.restoreAllMocks());

describe('controller binding rejects malformed actual identity', () => {
  it.each([
    { epoch: Number.NaN, incarnation: 'inc' },
    { epoch: -1, incarnation: 'inc' },
    { epoch: 1.5, incarnation: 'inc' },
    { epoch: Number.MAX_SAFE_INTEGER + 1, incarnation: 'inc' },
    { epoch: 1, incarnation: '' },
  ])('rejects $epoch / $incarnation with omitted caller constraints', ({ epoch, incarnation }) => {
    const store = new ConversationStore();
    let atom: ConversationAtom = {
      ...createInitialAtom(), conversation: { id: 'live' } as Conversation,
      conversationId: 'live', connectionState: 'live', connectionEpoch: epoch,
      streamIncarnation: incarnation, lastAppliedEventSeq: 10,
      streamingBuffer: { requestId: 'request', text: 'A', lastSequence: 10, startedAt: 0 },
    };
    let notify = () => {};
    vi.spyOn(store, 'slugForId').mockReturnValue('live');
    vi.spyOn(store, 'peekSnapshot').mockImplementation(() => atom);
    vi.spyOn(store, 'subscribeAny').mockImplementation(callback => {
      notify = callback;
      return () => { notify = () => {}; };
    });
    const receipts: ConsumptionReceipt[] = [];
    const controller = observeConsumption(store, 'live', receipt => { receipts.push(receipt); });
    controller.arm({ phase: 'drag', requestId: 'request', eventSequence: 11,
      tokenSequence: 11, text: 'AB' });
    expect(receipts.at(-1)).toMatchObject({ status: 'UNOBSERVED', reason: 'invalid actual connection identity' });
    atom = { ...atom, lastAppliedEventSeq: 11,
      streamingBuffer: { requestId: 'request', text: 'AB', lastSequence: 11, startedAt: 0 } };
    notify();
    expect(receipts.some(receipt => receipt.status === 'OBSERVED')).toBe(false);
    controller.dispose();
  });
});
