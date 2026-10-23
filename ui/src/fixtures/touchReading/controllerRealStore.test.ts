import { describe, expect, it } from 'vitest';
import { ConversationStore } from '../../conversation/ConversationStore';
import type { Conversation } from '../../api';
import type { InitPayload } from '../../conversation/atom';
import { observeConsumption, type ConsumptionReceipt } from './observeConsumption';

// Proposed reducer/store tests: no mocked snapshot/subscription and no React/SSE transport claim.
function setup() {
  const store = new ConversationStore();
  const payload: InitPayload = {
    conversation: { id: 'live', slug: 'live', model: 'fixture', cwd: '/tmp',
      created_at: '2024-01-01T00:00:00Z', updated_at: '2024-01-01T00:00:00Z', message_count: 0 } as Conversation,
    messages: [], opening: null, steeringMessages: [], phase: { type: 'llm_requesting', attempt: 1 },
    contextWindow: { used: 0 }, transcriptGeneration: 1, streamIncarnation: 'inc',
    lastAppliedEventSeq: 1, pendingAnchorSequenceId: 1, pendingEvents: [],
    pendingTruncated: false, transcriptCoverage: 'complete',
  };
  store.dispatch('live', { type: 'connection_opened', epoch: 7 });
  store.dispatch('live', { type: 'sse_init', epoch: 7, payload });
  store.dispatch('live', { type: 'connection_state', epoch: 7, state: 'live' });
  const receipts: ConsumptionReceipt[] = [];
  const observer = observeConsumption(store, 'live', receipt => { receipts.push(receipt); });
  const arm = (seq = 2, text = 'A') => observer.arm({ phase: 'drag', requestId: 'unique-request',
    eventSequence: seq, tokenSequence: seq, text, streamIncarnation: 'inc' });
  const token = (seq = 2, text = 'A', epoch = 7) => store.dispatch('live', {
    type: 'sse_token', epoch, sequenceId: seq, delta: text, requestId: 'unique-request',
  });
  return { store, observer, receipts, arm, token,
    observed: () => receipts.filter(receipt => receipt.status === 'OBSERVED') };
}

describe('actual-store controller connection binding', () => {
  it('diagnostic sampling is inert and notifications witness synchronous accepted reduction', () => {
    const r = setup();
    const count = r.receipts.length;
    const before = r.observer.diagnostic();
    expect(r.observer.diagnostic()).toEqual(before);
    expect(r.receipts).toHaveLength(count);
    expect(before.armed).toBe(false);
    r.arm();
    const armed = r.observer.diagnostic();
    expect(armed.armed).toBe(true);
    r.token(2, 'A', 6);
    expect(r.observer.diagnostic().notifications).toBe(armed.notifications);
    expect(r.observed()).toHaveLength(0);
    r.token();
    expect(r.observer.diagnostic().notifications).toBeGreaterThan(armed.notifications);
    expect(r.receipts.at(-1)).toMatchObject({ status: 'OBSERVED', eventSequence: 2 });
    r.observer.dispose();
  });
  it('binds actual epoch7 rather than fabricating epoch1 and requires a reduced token', () => {
    const r = setup();
    r.arm();
    expect(r.observed()).toEqual([]);
    r.token();
    expect(r.receipts.at(-1)).toMatchObject({ status: 'OBSERVED', connectionEpoch: 7,
      streamIncarnation: 'inc', eventSequence: 2, tokenSequence: 2, text: 'A' });
    expect(r.store.peekSnapshot('live')?.streamingBuffer?.text).toBe('A');
    r.observer.dispose();
  });
  it('rejects incorrect explicit expected connection identity', () => {
    const r = setup();
    r.observer.arm({ phase: 'drag', requestId: 'unique-request', eventSequence: 2,
      tokenSequence: 2, text: 'A', connectionEpoch: 1, streamIncarnation: 'inc' });
    r.token();
    expect(r.observed()).toEqual([]);
    r.observer.dispose();
  });
  it('cannot acknowledge a token consumed before arming', () => {
    const r = setup();
    r.token();
    r.arm();
    expect(r.observed()).toEqual([]);
    r.observer.dispose();
  });
  it('rejects stale-epoch wire actions and accepts a later current-epoch action', () => {
    const r = setup();
    r.arm();
    r.token(2, 'A', 6);
    expect(r.observed()).toEqual([]);
    expect(r.store.peekSnapshot('live')?.streamingBuffer).toBeNull();
    r.token();
    expect(r.observed()).toHaveLength(1);
    r.observer.dispose();
  });
  it('does not resurrect after actual out-of-order gap drains', () => {
    const r = setup();
    r.arm(3, 'AB');
    r.token(3, 'B');
    expect(r.store.peekSnapshot('live')?.eventGap).not.toBeNull();
    r.token(2, 'A');
    expect(r.store.peekSnapshot('live')?.eventGap).toBeNull();
    expect(r.store.peekSnapshot('live')?.streamingBuffer?.text).toBe('AB');
    expect(r.observed()).toEqual([]);
    r.arm(4, 'ABC');
    r.token(4, 'C');
    expect(r.receipts.at(-1)).toMatchObject({ status: 'OBSERVED', text: 'ABC' });
    r.observer.dispose();
  });
});
