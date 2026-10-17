import { describe, expect, it } from 'vitest';
import * as v from 'valibot';
import { ContinuationOpeningSchema, reconcileOpening } from './opening';
import type { ContinuationOpeningView } from '../generated/ContinuationOpeningView';
const edge = { product_conversation_id: 'p', predecessor_transcript_row_id: 'a', successor_transcript_row_id: 'b', continuation_message_id: 'c' };
const pending: ContinuationOpeningView = { status: 'pending', edge, client_turn_key: 'key', opening_authority: 'generated_predecessor_context' };
const accepted: ContinuationOpeningView = { status: 'accepted', edge, accepted_message_id: 'id', opening_authority: 'generated_predecessor_context', presentation: 'boundary' };
describe('opening authority validation', () => {
  it.each([pending, accepted, { status: 'none' }])('accepts actual generated shape %j', value => expect(v.safeParse(ContinuationOpeningSchema, value).success).toBe(true));
  it.each([null, {}, { status: 'none', edge }, { ...pending, client_turn_key: '' }, { ...pending, accepted_message_id: 'id' },
    { ...accepted, presentation: 'unknown' }, { ...accepted, opening_authority: 'unknown' }, { ...accepted, accepted_message_id: '' },
    ...Object.keys(edge).map(key => ({ ...pending, edge: { ...edge, [key]: '' } }))])('rejects malformed authority %j', value => expect(v.safeParse(ContinuationOpeningSchema, value).success).toBe(false));
  it('retains durable acceptance only for the same edge, and permits new presentation', () => {
    expect(reconcileOpening(accepted, pending)).toBe(accepted);
    expect(reconcileOpening(accepted, null)).toBe(accepted);
    const edited = { ...accepted, presentation: 'separate_message' as const };
    expect(reconcileOpening(accepted, edited)).toBe(edited);
    const different = { ...pending, edge: { ...edge, continuation_message_id: 'other' } };
    expect(reconcileOpening(accepted, different)).toBe(different);
  });
});
