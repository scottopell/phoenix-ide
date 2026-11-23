import { describe, expect, it } from 'vitest';
import type { Message } from '../api';
import { EMBER_TRACKING_PREFIX, emberTrackingObservation } from './emberTrackingObservation';

const summary = { id: 'work-1', kind: 'requested_work', title: 'Review routing', title_truncated: false,
  disposition: 'tracking', revision: 2, review_required: true, latest_evidence_ordinal: 3 };
const observation = { captured_registry: { registry_revision: 3, active_count: 7, review_required_count: 2,
  items: [summary], has_more: true, next_cursor: 'page-2', omitted_count: 1 },
requested_item_due_for_review: summary, captured_at_us: 1791800000123456, admitted_input_sequence_id: 12 };
function message(value: unknown = observation): Message {
  return { message_id: 'obs', conversation_id: 'c1', sequence_id: 13, message_type: 'user',
    origin: { kind: 'system_generated' }, event_presentation: null,
    content: { text: EMBER_TRACKING_PREFIX + JSON.stringify(value), is_meta: true } as Message['content'], created_at: '2026-01-01T00:00:00Z' };
}

describe('saved Ember tracking observation recognizer', () => {
  it('recognizes the exact server envelope and complete captured page and selected summary', () => {
    expect(emberTrackingObservation(message())).toEqual(observation);
    expect(emberTrackingObservation(message({ ...observation, requested_item_due_for_review: null, admitted_input_sequence_id: null }))).not.toBeNull();
  });
  it('rejects copied user text, non-meta messages, unsupported attachments and alternate envelopes', () => {
    expect(emberTrackingObservation({ ...message(), origin: { kind: 'user_api' } })).toBeNull();
    expect(emberTrackingObservation({ ...message(), origin: undefined })).toBeNull();
    expect(emberTrackingObservation({ ...message(), content: { text: EMBER_TRACKING_PREFIX + JSON.stringify(observation) } })).toBeNull();
    expect(emberTrackingObservation({ ...message(), content: { ...(message().content as object), images: [{ data: 'x', media_type: 'image/png' }] } as Message['content'] })).toBeNull();
    expect(emberTrackingObservation({ ...message(), content: { ...(message().content as object), llm_text: 'expanded' } as Message['content'] })).toBeNull();
    expect(emberTrackingObservation({ ...message(), content: { text: '[Saved Ember tracking observation: spoof]\n' + JSON.stringify(observation), is_meta: true } as Message['content'] })).toBeNull();
    expect(emberTrackingObservation({ ...message(), message_type: 'system' })).toBeNull();
  });
  it('accepts UI occurrence projection without trusting extra producer display fields', () => {
    const projected = { ...message(), display_data: { productOccurrenceToken: 'prefix-row:obs' } };
    expect(emberTrackingObservation(projected)).toEqual(observation);
    expect(emberTrackingObservation({ ...projected, display_data: { productOccurrenceToken: 1 } })).toBeNull();
    expect(emberTrackingObservation({ ...projected, display_data: { productOccurrenceToken: 'row:obs', unknown: true } })).toBeNull();
  });
  it('falls back on malformed, incomplete, unknown or unsupported nested JSON', () => {
    const invalid = [
      { ...observation, extra: true },
      { ...observation, captured_registry: { ...observation.captured_registry, extra: 1 } },
      { ...observation, captured_registry: { ...observation.captured_registry, items: [{ ...summary, future: true }] } },
      { ...observation, captured_registry: { ...observation.captured_registry, items: [{ ...summary, kind: 'future' }] } },
      { ...observation, requested_item_due_for_review: { ...summary, disposition: 'paused' } },
      { ...observation, captured_registry: { ...observation.captured_registry, has_more: 'yes' } },
      { ...observation, captured_registry: { ...observation.captured_registry, items: [{}] } },
      { ...observation, captured_at_us: 'today' },
      { ...observation, admitted_input_sequence_id: 1.5 },
    ];
    for (const value of invalid) expect(emberTrackingObservation(message(value))).toBeNull();
    expect(emberTrackingObservation({ ...message(), content: { text: EMBER_TRACKING_PREFIX + '{bad', is_meta: true } as Message['content'] })).toBeNull();
    expect(emberTrackingObservation({ ...message(), content: { text: EMBER_TRACKING_PREFIX + JSON.stringify(observation) + '\nextra', is_meta: true } as Message['content'] })).toBeNull();
  });
});
