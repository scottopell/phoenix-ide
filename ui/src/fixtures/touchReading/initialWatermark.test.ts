import { describe, expect, it } from 'vitest';
import { parse } from 'valibot';
import { SseInitDataSchema } from '../../sseSchemas';
import { ConversationStore } from '../../conversation/ConversationStore';
import type { InitPayload } from '../../conversation/atom';
import { observeConsumption, type ConsumptionReceipt } from './observeConsumption';
import { makeTouchScenario } from './scenario';
import { alignTouchInitialWatermark } from './initialWatermark';

function wire() {
  const scenario = makeTouchScenario();
  const messages = scenario.alignedLatestMessages;
  if (!messages) throw new Error('Missing aligned history');
  return parse(SseInitDataSchema, {
    opening: null, sequence_id: 1,
    conversation: { id: scenario.snapshot.latest_transcript_row_id, slug: scenario.snapshot.latest_transcript_row_id,
      title: 'fixture', model: 'fixture', cwd: '/fixture', created_at: '2024-01-01T00:00:00Z',
      updated_at: '2024-01-01T00:00:00Z', message_count: messages.length, state: scenario.latestConversationState },
    transcript_generation: 1, transcript_coverage: 'complete', messages, steering_messages: [],
    agent_working: false, last_sequence_id: 1, stream_incarnation: 'fixture-regression',
    presentation_mode: 'idle', context_window_size: 128000, project_name: null,
    pending_anchor_sequence_id: 1, pending_events: [], pending_truncated: false,
  });
}
describe('touch initial history watermark', () => {
  it('rejects ambiguous replay inputs', () => {
    const raw = wire();
    expect(() => alignTouchInitialWatermark({ ...raw, transcript_coverage: 'tail' })).toThrow();
    expect(() => alignTouchInitialWatermark({ ...raw, pending_truncated: true })).toThrow();
    expect(() => alignTouchInitialWatermark({ ...raw, pending_events: [{ type: 'token', sequence_id: 2, text: 'A', request_id: 'r' }] })).toThrow();
  });
  it('aligns nontrivial history and independently rejects stale/equal then accepts fresh reduction', () => {
    const raw = wire();
    const init = alignTouchInitialWatermark(raw);
    const floor = Math.max(...raw.messages.map(message => message.sequence_id));
    expect(floor).toBeGreaterThan(1);
    expect([init.sequence_id, init.last_sequence_id, init.pending_anchor_sequence_id]).toEqual([floor, floor, floor]);
    expect(raw.last_sequence_id).toBe(1);
    expect(init.messages).toBe(raw.messages);
    const store = new ConversationStore();
    const live = init.conversation.id;
    const payload: InitPayload = { conversation: init.conversation, messages: init.messages,
      opening: null, steeringMessages: [], phase: { type: 'llm_requesting', attempt: 1 },
      contextWindow: { used: 0 }, transcriptGeneration: init.transcript_generation,
      streamIncarnation: init.stream_incarnation, lastAppliedEventSeq: init.last_sequence_id,
      pendingAnchorSequenceId: init.pending_anchor_sequence_id, pendingEvents: init.pending_events,
      pendingTruncated: init.pending_truncated, transcriptCoverage: init.transcript_coverage };
    store.dispatch(live, { type: 'connection_opened', epoch: 7 });
    store.dispatch(live, { type: 'sse_init', epoch: 7, payload });
    store.dispatch(live, { type: 'connection_state', epoch: 7, state: 'live' });
    const receipts: ConsumptionReceipt[] = [];
    const observer = observeConsumption(store, live, receipt => receipts.push(receipt));
    expect(store.peekSnapshot(live)?.lastAppliedEventSeq).toBe(floor);
    for (const sequenceId of [floor - 1, floor]) {
      observer.arm({ phase: 'seed', requestId: 'stale', eventSequence: sequenceId, tokenSequence: sequenceId, text: 'A' });
      expect(receipts.at(-1)).toMatchObject({ status: 'UNOBSERVED', reason: 'target must require fresh post-arm consumption' });
      store.dispatch(live, { type: 'sse_token', epoch: 7, sequenceId, requestId: 'stale', delta: 'A' });
      expect(store.peekSnapshot(live)?.streamingBuffer).toBeNull();
      expect(store.peekSnapshot(live)?.lastAppliedEventSeq).toBe(floor);
    }
    observer.arm({ phase: 'seed', requestId: 'fresh', eventSequence: floor + 1, tokenSequence: floor + 1,
      streamIncarnation: init.stream_incarnation, connectionEpoch: 7, text: 'A' });
    store.dispatch(live, { type: 'sse_token', epoch: 7, sequenceId: floor + 1, requestId: 'fresh', delta: 'A' });
    expect(receipts.at(-1)).toMatchObject({ status: 'OBSERVED', connectionEpoch: 7,
      streamIncarnation: init.stream_incarnation, eventSequence: floor + 1, tokenSequence: floor + 1, requestId: 'fresh', text: 'A' });
    expect(store.peekSnapshot(live)?.streamingBuffer?.text).toBe('A');
    observer.dispose();
  });
});
