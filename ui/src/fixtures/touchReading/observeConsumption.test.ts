import { afterEach, describe, expect, it, vi } from 'vitest';
import { ConversationStore } from '../../conversation/ConversationStore';
import { createInitialAtom, type ConversationAtom } from '../../conversation/atom';
import type { Conversation } from '../../api';
import { observeConsumption, type ConsumptionReceipt, type ConsumptionTarget } from './observeConsumption';

// Destination: ui/src/fixtures/touchReading/observeConsumption.test.ts.
// Observer unit tests with controlled reduced snapshots, NOT reducer/SSE or layout qualification.
function rig() {
  const store = new ConversationStore();
  let key = 'live-key';
  let atom: ConversationAtom | undefined = {
    ...createInitialAtom(), conversation: { id: 'live' } as Conversation,
    conversationId: 'live', connectionEpoch: 1, streamIncarnation: 'inc-1',
    connectionState: 'live', lastAppliedEventSeq: 10,
    streamingBuffer: { requestId: 'req', lastSequence: 1, text: 'A', startedAt: 0 },
  };
  let notify = () => {};
  const unsubscribe = vi.fn(() => { notify = () => {}; });
  vi.spyOn(store, 'slugForId').mockImplementation(id => id === 'live' ? key : undefined);
  vi.spyOn(store, 'peekSnapshot').mockImplementation(candidate => candidate === key ? atom : undefined);
  vi.spyOn(store, 'subscribeAny').mockImplementation(callback => { notify = callback; return unsubscribe; });
  const creatingRead = vi.spyOn(store, 'getSnapshot');
  const dispatch = vi.spyOn(store, 'dispatch');
  const receipts: ConsumptionReceipt[] = [];
  const controller = observeConsumption(store, 'live', receipt => { receipts.push(receipt); });
  const target = (overrides: Partial<ConsumptionTarget> = {}): ConsumptionTarget => ({
    phase: 'drag', requestId: 'req', eventSequence: 11, tokenSequence: 2,
    text: 'AB', connectionEpoch: 1, streamIncarnation: 'inc-1', ...overrides,
  });
  return {
    controller, receipts, target, unsubscribe, creatingRead, dispatch,
    tick: () => notify(),
    set: (changes: Partial<ConversationAtom>) => { atom = { ...atom!, ...changes }; notify(); },
    consume: (event = 11, token = 2, text = 'AB') => {
      atom = { ...atom!, lastAppliedEventSeq: event,
        streamingBuffer: { requestId: 'req', lastSequence: token, text, startedAt: 0 } };
      notify();
    },
    rekey: () => { key = 'new-key'; notify(); },
    remove: () => { atom = undefined; notify(); },
    observed: () => receipts.filter(receipt => receipt.status === 'OBSERVED'),
    last: () => receipts.at(-1),
  };
}

afterEach(() => vi.restoreAllMocks());

describe('phase-causal consumption observer', () => {
  it('rejects already-present state, including disarm/rearm under a different phase', () => {
    const r = rig();
    r.controller.arm(r.target({ eventSequence: 10, tokenSequence: 1, text: 'A' }));
    r.controller.disarm();
    r.controller.arm(r.target({ phase: 'hold', eventSequence: 10, tokenSequence: 1, text: 'A' }));
    r.tick();
    expect(r.observed()).toEqual([]);
    expect(r.last()?.status).toBe('UNOBSERVED');
    r.controller.dispose();
  });

  it('requires actual post-arm token progression, not an unrelated event advancing the watermark', () => {
    const r = rig();
    r.controller.arm(r.target({ tokenSequence: 1, text: 'A' }));
    r.set({ lastAppliedEventSeq: 11 });
    expect(r.observed()).toEqual([]);
    r.controller.arm(r.target({ eventSequence: 12 }));
    r.tick();
    expect(r.observed()).toEqual([]);
    r.consume(12);
    expect(r.last()).toMatchObject({ status: 'OBSERVED', phase: 'drag', eventSequence: 12, tokenSequence: 2, text: 'AB' });
    expect(r.creatingRead).not.toHaveBeenCalled();
    expect(r.dispatch).not.toHaveBeenCalled();
    r.controller.dispose();
  });

  it('rejects event-before-arm but accepts arm-before-event', () => {
    const r = rig();
    r.consume();
    r.controller.arm(r.target());
    expect(r.observed()).toEqual([]);
    r.controller.arm(r.target({ eventSequence: 12, tokenSequence: 3, text: 'ABC' }));
    r.consume(12, 3, 'ABC');
    expect(r.observed()).toHaveLength(1);
    r.controller.dispose();
  });

  it('invalidates the former receipt before sampling a new phase', () => {
    const r = rig();
    r.controller.arm(r.target());
    r.consume();
    expect(r.observed()).toHaveLength(1);
    const offset = r.receipts.length;
    r.controller.arm(r.target({ phase: 'hold', eventSequence: 12, tokenSequence: 3, text: 'ABC' }));
    expect(r.receipts.slice(offset).every(receipt => receipt.status === 'UNOBSERVED')).toBe(true);
    expect(r.receipts[offset]).toMatchObject({ status: 'UNOBSERVED', phase: null });
    r.consume(12, 3, 'ABC');
    expect(r.last()).toMatchObject({ status: 'OBSERVED', phase: 'hold' });
    r.controller.dispose();
  });

  it('never resurrects a gap-invalidated phase on gap clear; rearm requires future consumption', () => {
    const r = rig();
    r.controller.arm(r.target());
    r.set({ eventGap: { expectedNextEventSeq: 11, firstBufferedEventSeq: 12 } });
    r.consume();
    r.set({ eventGap: null });
    r.tick();
    expect(r.observed()).toEqual([]);
    r.controller.arm(r.target());
    expect(r.observed()).toEqual([]);
    r.controller.arm(r.target({ eventSequence: 12, tokenSequence: 3, text: 'ABC' }));
    r.consume(12, 3, 'ABC');
    expect(r.observed()).toHaveLength(1);
    r.controller.dispose();
  });

  it.each(['alias', 'epoch', 'incarnation', 'missing', 'reconnect'] as const)('invalidates on %s, even when matching state returns', kind => {
    const r = rig();
    r.controller.arm(r.target());
    if (kind === 'alias') r.rekey();
    if (kind === 'epoch') { r.set({ connectionEpoch: 2 }); r.set({ connectionEpoch: 1 }); }
    if (kind === 'incarnation') { r.set({ streamIncarnation: 'inc-2' }); r.set({ streamIncarnation: 'inc-1' }); }
    if (kind === 'missing') r.remove();
    if (kind === 'reconnect') { r.set({ connectionState: 'reconnecting' }); r.set({ connectionState: 'live' }); }
    if (kind !== 'missing') r.consume();
    r.tick();
    expect(r.observed()).toEqual([]);
    expect(r.last()?.status).toBe('UNOBSERVED');
    r.controller.dispose();
  });

  it('accepts coalesced exact final state but rejects mismatched text', () => {
    const r = rig();
    r.controller.arm(r.target());
    r.consume(13, 2, 'WRONG');
    expect(r.observed()).toEqual([]);
    r.consume(14, 2, 'AB');
    expect(r.last()).toMatchObject({ status: 'OBSERVED', eventSequence: 14, tokenSequence: 2, text: 'AB' });
    r.controller.dispose();
  });

  it('disposes once, invalidates receipt and ignores notifications and subsequent arming', () => {
    const r = rig();
    r.controller.arm(r.target());
    r.consume();
    r.controller.dispose();
    const count = r.receipts.length;
    r.controller.dispose();
    r.consume(12, 3, 'ABC');
    r.controller.arm(r.target({ eventSequence: 13 }));
    expect(r.unsubscribe).toHaveBeenCalledTimes(1);
    expect(r.receipts).toHaveLength(count);
    expect(r.last()).toMatchObject({ status: 'UNOBSERVED', phase: null, reason: 'observer disposed' });
  });
});
