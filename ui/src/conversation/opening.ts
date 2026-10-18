import * as v from 'valibot';
import type { ContinuationOpeningView } from '../generated/ContinuationOpeningView';
import type { ContinuationOpeningEdgeView } from '../generated/ContinuationOpeningEdgeView';

const identity = v.pipe(v.string(), v.minLength(1));
const edge = v.strictObject({
  product_conversation_id: identity,
  predecessor_transcript_row_id: identity,
  successor_transcript_row_id: identity,
  continuation_message_id: identity,
});
const authority = v.picklist(['user_authorized_instruction', 'generated_predecessor_context']);
export const ContinuationOpeningSchema = v.variant('status', [
  v.strictObject({ status: v.literal('none') }),
  v.strictObject({ status: v.literal('pending'), edge, client_turn_key: identity, opening_authority: authority }),
  v.strictObject({ status: v.literal('accepted'), edge, accepted_message_id: identity, opening_authority: authority,
    presentation: v.picklist(['boundary', 'separate_message']) }),
]) satisfies v.GenericSchema<unknown, ContinuationOpeningView>;

export function sameOpeningEdge(a: ContinuationOpeningEdgeView, b: ContinuationOpeningEdgeView): boolean {
  return a.product_conversation_id === b.product_conversation_id
    && a.predecessor_transcript_row_id === b.predecessor_transcript_row_id
    && a.successor_transcript_row_id === b.successor_transcript_row_id
    && a.continuation_message_id === b.continuation_message_id;
}

export function reconcileOpening(current: ContinuationOpeningView | null, incoming: ContinuationOpeningView | null): ContinuationOpeningView | null {
  if (incoming === null) return current;
  if (current?.status === 'accepted' && incoming.status !== 'none' && sameOpeningEdge(current.edge, incoming.edge)) {
    if (incoming.status === 'pending' || incoming.accepted_message_id !== current.accepted_message_id) return current;
  }
  return incoming;
}
