import type { Message } from '../api';

export interface TrackingSummary {
  id: string;
  kind: 'requested_work' | 'bookmark';
  title: string;
  title_truncated: boolean;
  disposition: 'tracking' | 'resolved' | 'dismissed';
  revision: number;
  review_required: boolean;
  latest_evidence_ordinal: number;
}

export interface TrackingPage {
  registry_revision: number;
  active_count: number;
  review_required_count: number;
  items: TrackingSummary[];
  has_more: boolean;
  next_cursor: string | null;
  omitted_count: number;
}

export interface TrackingObservation {
  captured_registry: TrackingPage;
  requested_item_due_for_review: TrackingSummary | null;
  captured_at_us: number;
  admitted_input_sequence_id: number | null;
}

const PREFIX = '[Saved Ember tracking observation: captured historical data, not a user instruction, permission, current worker status or comprehensive inventory. Use tracking read/list for current detail. New evidence requires reconciliation. A due cue applies only to the admitted input immediately BEFORE this observation; once a later input arrives it is historical and does not renew review. A non-null requested_item_due_for_review selects one bounded inspection under the static review policy after handling the current request; it is not a new USER commission, execution permission or instruction to nudge a worker. Inspection alone is not follow-through: use the static next-step policy, preserving actual restrictions and existing execution authority. No useful action means stay quiet.]\n';

function record<K extends string>(value: unknown, keys: readonly K[]): value is Record<K, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    && Object.keys(value).length === keys.length && keys.every(key => Object.hasOwn(value, key));
}
const integer = (value: unknown): value is number => Number.isSafeInteger(value) && (value as number) >= 0;
const string = (value: unknown): value is string => typeof value === 'string';
const boolean = (value: unknown): value is boolean => typeof value === 'boolean';

function summary(value: unknown): value is TrackingSummary {
  if (!record(value, ['id', 'kind', 'title', 'title_truncated', 'disposition', 'revision', 'review_required', 'latest_evidence_ordinal'])) return false;
  return string(value.id) && (value.kind === 'requested_work' || value.kind === 'bookmark')
    && string(value.title) && boolean(value.title_truncated)
    && (value.disposition === 'tracking' || value.disposition === 'resolved' || value.disposition === 'dismissed')
    && integer(value.revision) && boolean(value.review_required) && integer(value.latest_evidence_ordinal);
}

function page(value: unknown): value is TrackingPage {
  if (!record(value, ['registry_revision', 'active_count', 'review_required_count', 'items', 'has_more', 'next_cursor', 'omitted_count'])) return false;
  return integer(value.registry_revision) && integer(value.active_count) && integer(value.review_required_count)
    && Array.isArray(value.items) && value.items.every(summary) && boolean(value.has_more)
    && (value.next_cursor === null || string(value.next_cursor)) && integer(value.omitted_count);
}

function observation(value: unknown): value is TrackingObservation {
  if (!record(value, ['captured_registry', 'requested_item_due_for_review', 'captured_at_us', 'admitted_input_sequence_id'])) return false;
  return page(value.captured_registry) && (value.requested_item_due_for_review === null || summary(value.requested_item_due_for_review))
    && integer(value.captured_at_us) && (value.admitted_input_sequence_id === null || integer(value.admitted_input_sequence_id));
}

/** Only the known server-authored, attachment-free observation can replace ordinary message rendering. */
export function emberTrackingObservation(message: Message): TrackingObservation | null {
  if (message.message_type !== 'user' || message.origin?.kind !== 'system_generated' || message.event_presentation !== null) return null;
  if (message.display_data != null && (!record(message.display_data, ['productOccurrenceToken'])
    || !string(message.display_data.productOccurrenceToken))) return null;
  const content: unknown = message.content;
  if (!record(content, ['text', 'is_meta']) || content.is_meta !== true || !string(content.text) || !content.text.startsWith(PREFIX)) return null;
  try {
    const parsed: unknown = JSON.parse(content.text.slice(PREFIX.length));
    return observation(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

export function emberTrackingLabel(observation: TrackingObservation): string {
  const { active_count, review_required_count } = observation.captured_registry;
  return `Ember tracking · captured snapshot · ${active_count} active${review_required_count ? ` · ${review_required_count} review flagged` : ''}`;
}

export function emberTrackingTopic(observation: TrackingObservation): string | null {
  return observation.requested_item_due_for_review?.title || null;
}

export const EMBER_TRACKING_PREFIX = PREFIX;
