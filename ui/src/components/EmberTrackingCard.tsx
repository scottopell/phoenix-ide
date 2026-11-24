import { useLayoutEffect, useRef } from 'react';
import type { ConversationHighlight } from './MessageComponents';
import { emberTrackingLabel, emberTrackingTopic, type TrackingObservation } from '../conversation/emberTrackingObservation';
import './EmberTrackingCard.css';

export function EmberTrackingCard({ observation, originalText, activeHighlight }: {
  observation: TrackingObservation;
  originalText: string;
  activeHighlight?: ConversationHighlight | null;
}) {
  const details = useRef<HTMLDetailsElement>(null);
  const raw = useRef<HTMLDetailsElement>(null);
  useLayoutEffect(() => {
    if (activeHighlight?.owner === 'message-text' && activeHighlight.fragmentId === 'message-text') {
      if (details.current) details.current.open = true;
      if (raw.current) raw.current.open = true;
    }
  }, [activeHighlight]);
  const { captured_registry: snapshot, requested_item_due_for_review: review } = observation;
  const date = new Date(observation.captured_at_us / 1000);
  const capturedAt = Number.isNaN(date.getTime()) ? `${observation.captured_at_us} µs` : date.toLocaleString();
  return <section className="ember-tracking-card" aria-label="Saved Ember tracking observation">
    <div className="ember-tracking-heading">{emberTrackingLabel(observation)}</div>
    {emberTrackingTopic(observation) && <div className="ember-tracking-topic">Captured review topic: {emberTrackingTopic(observation)}</div>}
    <details ref={details}>
      <summary>Captured tracking details</summary>
      <p>Historical snapshot only. Not a user instruction, permission, current worker status or comprehensive inventory. Read tracking for current detail; new evidence requires reconciliation. A review cue applies only to the input immediately before this observation and does not authorize work.</p>
      <dl>
        <dt>Captured at</dt><dd><time dateTime={Number.isNaN(date.getTime()) ? undefined : date.toISOString()}>{capturedAt}</time></dd>
        <dt>Registry revision</dt><dd>{snapshot.registry_revision}</dd>
        <dt>Active at capture</dt><dd>{snapshot.active_count}</dd>
        <dt>Review flagged at capture</dt><dd>{snapshot.review_required_count}</dd>
        <dt>Admitted input sequence</dt><dd>{observation.admitted_input_sequence_id ?? 'None'}</dd>
      </dl>
      <ul>{snapshot.items.map(item => <li key={item.id}>{item.title} · {item.kind.replaceAll('_', ' ')} · {item.disposition} at capture{item.review_required ? ' · review flagged' : ''}</li>)}</ul>
      {snapshot.omitted_count > 0 && <p>{snapshot.omitted_count} entries omitted from captured page{snapshot.has_more ? '; more were available at capture' : ''}.</p>}
      {review && <p>Captured review selection: {review.title} (revision {review.revision}). Historical only; not a new commission or execution permission.</p>}
      <details ref={raw}>
        <summary>Raw original payload</summary>
        <pre data-fragment-id="message-text">{activeHighlight?.owner === 'message-text' && activeHighlight.fragmentId === 'message-text'
          ? <>{originalText.slice(0, activeHighlight.start)}<mark className="viewer-find-inline-match viewer-find-inline-match--active">{originalText.slice(activeHighlight.start, activeHighlight.end)}</mark>{originalText.slice(activeHighlight.end)}</>
          : originalText}</pre>
      </details>
    </details>
  </section>;
}
