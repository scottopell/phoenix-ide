import { expect, it } from 'vitest';
import type { Conversation } from '../../api';
import { ConversationStore } from '../../conversation/ConversationStore';
import { observeConsumption, type ConsumptionReceipt } from './observeConsumption';
import { continuationTarget } from './continuationTarget';

function seeded() {
  const store = new ConversationStore();
  store.dispatch('append-fixture', { type: 'connection_opened', epoch: 9 });
  store.dispatch('append-fixture', { type: 'sse_init', epoch: 9, payload: {
    conversation: { id: 'append-fixture', slug: 'append-fixture', model: 'test', cwd: '/tmp',
      created_at: '2024-01-01T00:00:00Z', updated_at: '2024-01-01T00:00:00Z', message_count: 0 } as Conversation,
    messages: [], opening: null, steeringMessages: [], phase: { type: 'llm_requesting', attempt: 1 },
    contextWindow: { used: 0 }, transcriptGeneration: 1, streamIncarnation: 'append-incarnation',
    lastAppliedEventSeq: 1, pendingAnchorSequenceId: 1, pendingEvents: [], pendingTruncated: false, transcriptCoverage: 'complete',
  } });
  store.dispatch('append-fixture', { type: 'connection_state', epoch: 9, state: 'live' });
  const receipts: ConsumptionReceipt[] = [];
  const observer = observeConsumption(store, 'append-fixture', r => receipts.push(r));
  observer.arm({ phase: 'seed', requestId: 'same-request', streamIncarnation: 'append-incarnation', connectionEpoch: 9,
    eventSequence: 2, tokenSequence: 2, text: 'Seed. ' });
  store.dispatch('append-fixture', { type: 'sse_token', epoch: 9, sequenceId: 2, requestId: 'same-request', delta: 'Seed. ' });
  const receipt = receipts.at(-1)!;
  expect(receipt.status).toBe('OBSERVED');
  const input = { receipt, phase: 'append', tokens: ['More. ', 'End.'], liveId: 'append-fixture', requestId: 'same-request',
    streamIncarnation: 'append-incarnation', sequence: 2, text: 'Seed. ' };
  return { store, observer, receipts, input };
}

it('actual reducer retains request and cumulatively consumes fresh same-request tokens', () => {
  const f = seeded();
  try {
    const before = f.store.peekSnapshot('append-fixture')!.streamingBuffer!;
    const target = continuationTarget(f.input);
    f.observer.arm(target);
    expect(f.receipts.at(-1)?.status).toBe('UNOBSERVED');
    f.store.dispatch('append-fixture', { type: 'sse_token', epoch: 9, sequenceId: 3, requestId: 'same-request', delta: 'More. ' });
    expect(f.receipts.at(-1)?.status).toBe('UNOBSERVED');
    f.store.dispatch('append-fixture', { type: 'sse_token', epoch: 9, sequenceId: 4, requestId: 'same-request', delta: 'End.' });
    expect(f.receipts.at(-1)).toMatchObject({ status: 'OBSERVED', requestId: 'same-request', connectionEpoch: 9,
      streamIncarnation: 'append-incarnation', eventSequence: 4, tokenSequence: 4, text: 'Seed. More. End.' });
    expect(f.store.peekSnapshot('append-fixture')!.streamingBuffer).toMatchObject({ requestId: before.requestId, startedAt: before.startedAt, text: target.text });
  } finally { f.observer.dispose(); }
});

it.each([
  ['foreign request', { requestId: 'other' }],
  ['foreign transcript', { liveId: 'other' }],
  ['foreign incarnation', { streamIncarnation: 'other' }],
  ['stale sequence', { sequence: 1 }],
  ['altered cumulative prefix', { text: 'Different. ' }],
  ['empty continuation', { tokens: [] }],
] as const)('rejects %s before producing a target', (_label, changes) => {
  const f = seeded();
  try { expect(() => continuationTarget({ ...f.input, ...changes })).toThrow(); }
  finally { f.observer.dispose(); }
});

it.each(['foreign', 'stale-epoch', 'replaced-request'] as const)('actual store does not acknowledge %s continuation', kind => {
  const f = seeded();
  try {
    const target = continuationTarget(f.input);
    if (kind === 'replaced-request') f.store.dispatch('append-fixture', { type: 'sse_token', epoch: 9,
      sequenceId: 3, requestId: 'replacement', delta: 'Replacement.' });
    f.observer.arm(target);
    const steps = kind === 'replaced-request' ? [{ sequenceId: 4, delta: 'More. End.' }]
      : [{ sequenceId: 3, delta: 'More. ' }, { sequenceId: 4, delta: 'End.' }];
    for (const step of steps) f.store.dispatch('append-fixture', { type: 'sse_token', epoch: kind === 'stale-epoch' ? 8 : 9,
      ...step, requestId: kind === 'foreign' ? 'foreign' : 'same-request' });
    expect(f.receipts.at(-1)?.status).toBe('UNOBSERVED');
  } finally { f.observer.dispose(); }
});
