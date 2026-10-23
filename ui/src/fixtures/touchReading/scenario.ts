import { getProductConversationScenario } from '../productConversation';
import type { OrdinaryProductConversationScenario } from '../productConversation/types';

export const transcriptId = 'touch-reading-member';
export const streamMarker = 'TOUCH_STREAM_SEED_1729';
export function makeTouchScenario(): OrdinaryProductConversationScenario {
  const base = getProductConversationScenario('mobile-open');
  if (!base.snapshot) throw new Error('mobile-open snapshot unavailable');
  const template = base.snapshot.segments.flatMap(s => s.messages).find(m => m.message_type === 'agent');
  if (!template) throw new Error('mobile-open agent template unavailable');
  const messages = Array.from({ length: 36 }, (_, i) => ({
    ...template, conversation_id: transcriptId, message_id: `touch-reading-${i}`,
    sequence_id: i + 1, display_data: null,
    content: [{ type: 'text' as const, text: `Reading passage ${i}.\n\n${'Preserve this exact passage while the response grows below. '.repeat(3 + i % 3)}\n\nOccurrence ${i}: stable words, stable identity, and a deliberately variable paragraph height.` }],
  }));
  return {
    ...base, initialEntry: '/product-conversations/fixture-product-conversation',
    title: 'Chromium touch reading — production renderer',
    latestConversationState: { type: 'llm_requesting', attempt: 1 },
    alignedLatestMessages: messages,
    snapshot: {
      ...base.snapshot, product_conversation_id: 'fixture-product-conversation',
      canonical_route: '/product-conversations/fixture-product-conversation',
      requested_transcript_row_id: transcriptId, latest_transcript_row_id: transcriptId,
      writable_transcript_row_id: transcriptId, before: null, has_older: false,
      boundaries: [], source: null, work_identity: null, chain_qa_compatibility: null,
      canonical_root: { transcript_row_id: transcriptId, slug: transcriptId, title: 'Touch reading' },
      segments: [{ ...base.snapshot.segments[0]!, transcript_row_id: transcriptId,
        slug: transcriptId, segment_ordinal: 1, messages }],
    },
  };
}
