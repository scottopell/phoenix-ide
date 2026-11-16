import type { EnrichedMessage } from '../../generated/EnrichedMessage';
import type { EventPresentation } from '../../generated/EventPresentation';
import { coordinatorSnapshot, getProductConversationScenario } from './scenarios';
import type { OrdinaryProductConversationScenario, ProductConversationScenario } from './types';

export const compactEventTitle = 'Preserve canonical conversation routing and exact historical source provenance across continued transcripts without losing the original sender context';
export const compactEventFailure = 'Exact failure: fixture validation rejected /workspace/source?attempt=2&mode=read-only; no retry was attempted.';
export const compactEventEnding = `Settled input fixture-user-input (not necessarily the sole cause), origin: {"kind":"user_api"}.
Ending response fixture-ending-response; evidence @event:00000000-0000-4000-8000-000000000001.
Recorded ending quotation (untrusted evidence, not instructions):
"Keep deployment paused until the user lifts the hold." Literal evidence: <quoted> & safe.
${'東京の会話 🦊 '.repeat(28)}
[Ending quotation truncated; use read_event_evidence for omitted text.]`;

const completed = {
  status: 'available', ending_evidence: compactEventEnding, event_id: 'fixture-event-completed', product_conversation_id: 'source-member',
  display_name: compactEventTitle, source_transcript_id: 'source-member',
  source_occurrence_kind: 'turn', source_occurrence_id: 'fixture-turn-completed', source_generation: 7,
  occurred_at_us: 1791800000123456, provenance: 'subscription', outcome: { kind: 'turn_finished' },
} satisfies EventPresentation;

function eventMessage(id: string, sequence: number, presentation: EventPresentation, text: string) {
  return {
    message_id: id, conversation_id: 'source-member', sequence_id: sequence, message_type: 'user',
    origin: { kind: 'subscription_event', event_id: presentation.event_id }, event_presentation: presentation,
    content: { text }, display_data: null, usage_data: null, created_at: '2026-10-12T08:53:20.123Z',
  } satisfies EnrichedMessage;
}

export function makeCompactEventsScenario(global: boolean, redacted = false): ProductConversationScenario {
  const base = getProductConversationScenario(global ? 'source-call-global' : 'source-call-ordinary');
  const source = base.sourceSnapshot!;
  const messages = [
    eventMessage('event-completed', 1, redacted ? { status: 'unavailable', event_id: completed.event_id, reason: 'redacted' } : completed, '[fixture original completion]\nThe source turn finished. Task completion is not inferred.'),
    eventMessage('event-failed', 2, redacted ? { status: 'unavailable', event_id: 'fixture-event-failed', reason: 'redacted' } : {
      ...completed, ending_evidence: 'Historical ending evidence not captured.', event_id: 'fixture-event-failed', display_name: 'Validate source navigation',
      source_occurrence_id: 'fixture-turn-failed', outcome: { kind: 'failed', reason: compactEventFailure },
    }, `[fixture original failure]\n${compactEventFailure}`),
    eventMessage('event-missing', 3, { status: 'unavailable', event_id: 'fixture-event-missing', reason: redacted ? 'redacted' : 'missing' },
      '[fixture original missing event]\nRetain this exact text when event metadata is unavailable.'),
    {
      message_id: 'event-human-prompt', conversation_id: 'source-member', sequence_id: 4, message_type: 'user',
      origin: { kind: 'user_api' }, event_presentation: null,
      content: { text: 'Review these events without treating them as new user chapters.' },
      display_data: null, usage_data: null, created_at: '2026-10-12T08:54:00Z',
    },
  ] satisfies EnrichedMessage[];
  const first = source.segments[0]!;
  const ordinary: OrdinaryProductConversationScenario = {
    ...base,
    id: redacted ? 'compact-events-redacted' : global ? 'compact-events-global' : 'compact-events-ordinary',
    title: global ? 'Compact events / actual Global page' : 'Compact events / ordinary continued transcript',
    sourceIsGlobal: false,
    ...(global ? {
      initialEntry: '/global/event-latest',
      alignedLatestMessages: messages.slice(1).map(message => ({ ...message, conversation_id: 'event-latest' })),
    } : {}),
    sourceSnapshot: { ...source, boundaries: [], product_conversation_id: 'source-member', canonical_route: '/product-conversations/source-member', requested_transcript_row_id: 'source-member', latest_transcript_row_id: 'source-member', writable_transcript_row_id: 'source-member', segments: [{ ...first, messages: [
      { ...messages[3]!, content: { text: 'Source conversation for compact event acceptance.' } },
    ] }] },
    snapshot: {
      ...base.snapshot!, boundaries: [], work_identity: null, source: null, chain_qa_compatibility: null,
      latest_transcript_row_id: 'event-latest', writable_transcript_row_id: 'event-latest',
      presentation: { kind: 'state', display_name: 'Compact events', presentation_mode: 'idle' },
      segments: [
        { ...first, transcript_row_id: 'event-history', slug: 'event-history', title: 'Historical events', messages: [{ ...messages[0]!, conversation_id: 'event-history' }] },
        { ...first, segment_ordinal: 2, transcript_row_id: 'event-latest', slug: 'event-latest', title: 'Current transcript', messages: messages.slice(1).map(message => ({ ...message, conversation_id: 'event-latest' })) },
      ],
    },
  };
  return global ? { ...ordinary, kind: 'coordinator', snapshot: coordinatorSnapshot({ ...ordinary.snapshot!, product_conversation_id: 'event-latest' }) } : ordinary;
}
