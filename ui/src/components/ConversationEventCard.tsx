import { useLayoutEffect, useRef, type ReactNode } from 'react';
import { Link as RouterLink, useLocation } from 'react-router-dom';
import type { EventPresentation } from '../generated/EventPresentation';
import type { ConversationHighlight } from './MessageComponents';
import { CopyButton } from './CopyButton';
import { coordinatorConversationLinkState } from './conversationReturnOrigin';
import './ConversationEventCard.css';

// eslint-disable-next-line react-refresh/only-export-components
export function eventCardFragments(presentation: EventPresentation | null, eventId: string) {
  if (!presentation || presentation.status === 'unavailable') {
    return { title: 'Conversation event', outcome: 'Details unavailable', time: '', provenance: `Event ID: ${presentation?.event_id ?? eventId}${presentation ? `\nUnavailable: ${presentation.reason}` : ''}`, reason: '' };
  }
  if (presentation.status === 'federated_gap') {
    const current = presentation.current.kind === 'terminal'
      ? `Terminal (${presentation.current.outcome})` : 'Nonterminal';
    return {
      title: `Remote watch gap · ${presentation.source_instance_id}`,
      outcome: `Gap: ${presentation.reason.replaceAll('_', ' ')} · ${current}`,
      time: new Date(presentation.failed_at_us / 1000).toLocaleString(),
      reason: `Current state: ${current}. Terminal evidence expires ${new Date(presentation.terminal_evidence_expires_at_us / 1000).toLocaleString()}. Read current state and create a new subscription.`,
      provenance: [
        `Event ID: ${presentation.event_id}`, `Source instance: ${presentation.source_instance_id}`,
        `Source conversation: ${presentation.source_product_conversation_id}`, `Subscription ID: ${presentation.subscription_id}`,
        `Transcript ID: ${presentation.source_transcript_id}`, `Gap reason: ${presentation.reason}`,
        `Last acknowledged: ${presentation.last_acknowledged}`, `Last produced: ${presentation.last_produced}`,
        `Failed at (microseconds): ${presentation.failed_at_us}`,
        `Terminal evidence expires (microseconds): ${presentation.terminal_evidence_expires_at_us}`,
        `Disposition: ${JSON.stringify(presentation.disposition)}`,
      ].join('\n'),
    };
  }
  if (presentation.status === 'federated_fact') {
    return {
      title: `Remote conversation · ${presentation.source_instance_id}`,
      outcome: `Remote ${presentation.outcome}`,
      time: new Date(presentation.occurred_at_us / 1000).toLocaleString(),
      reason: '',
      provenance: [
        `Event ID: ${presentation.event_id}`, `Source instance: ${presentation.source_instance_id}`,
        `Source conversation: ${presentation.source_product_conversation_id}`, `Subscription ID: ${presentation.subscription_id}`,
        `Source event ID: ${presentation.source_event_id}`, `Sequence: ${presentation.sequence}`,
        `Transcript ID: ${presentation.source_transcript_id}`, `Occurrence kind: ${presentation.source_occurrence_kind}`,
        `Occurrence ID: ${presentation.source_occurrence_id}`, `Generation: ${presentation.source_generation}`,
        `Time (microseconds): ${presentation.occurred_at_us}`, `Disposition: ${JSON.stringify(presentation.disposition)}`,
      ].join('\n'),
    };
  }
  const outcome = presentation.outcome;
  const labels = {
    turn_finished: 'Turn finished', failed: 'Failed', cleanup_failed: 'Cleanup failed',
    cancelled: 'Cancelled', awaiting_user_response: 'Awaiting user response', awaiting_task_approval: 'Awaiting task approval',
    goal_changed: 'Goal changed',
  };
  const date = new Date(presentation.occurred_at_us / 1000);
  return {
    title: presentation.display_name?.trim() ? presentation.display_name : 'Untitled conversation',
    outcome: labels[outcome.kind],
    time: date.toLocaleString(),
    reason: outcome.kind === 'failed' || outcome.kind === 'cleanup_failed' ? outcome.reason : '',
    provenance: [
      `Event ID: ${presentation.event_id}`, `Conversation ID: ${presentation.product_conversation_id}`,
      `Transcript ID: ${presentation.source_transcript_id}`, `Occurrence kind: ${presentation.source_occurrence_kind}`,
      `Occurrence ID: ${presentation.source_occurrence_id}`, `Generation: ${presentation.source_generation}`,
      `Time (microseconds): ${presentation.occurred_at_us}`, `Provenance: ${presentation.provenance}`,
    ].join('\n'),
  };
}

export function ConversationEventCard({ presentation, eventId, originalText, activeHighlight, children }: {
  presentation: EventPresentation | null;
  eventId: string;
  originalText: string;
  activeHighlight?: ConversationHighlight | null;
  children: ReactNode;
}) {
  const location = useLocation();
  const details = useRef<HTMLDetailsElement>(null);
  const fragments = eventCardFragments(presentation, eventId);
  const available = presentation?.status === 'available' ? presentation : null;
  const href = available ? `/c/${encodeURIComponent(available.product_conversation_id)}` : null;
  const remoteHref = presentation?.status === 'federated_fact' && presentation.navigation.status === 'available'
    ? presentation.navigation.stable_url : null;
  const timeUs = available?.occurred_at_us ?? (presentation?.status === 'federated_fact' ? presentation.occurred_at_us
    : presentation?.status === 'federated_gap' ? presentation.failed_at_us : null);
  useLayoutEffect(() => {
    if (activeHighlight && details.current && (
      activeHighlight.fragmentId === 'event-provenance' || activeHighlight.fragmentId === 'event-reason'
      || activeHighlight.fragmentId === 'message-text' || activeHighlight.owner === 'message-attachment'
    )) details.current.open = true;
  }, [activeHighlight]);
  function fragment(id: string, text: string) {
    const highlight = activeHighlight?.fragmentId === id ? activeHighlight : null;
    return <span data-fragment-id={id}>{highlight
      ? <>{text.slice(0, highlight.start)}<mark className="viewer-find-inline-match viewer-find-inline-match--active">{text.slice(highlight.start, highlight.end)}</mark>{text.slice(highlight.end)}</>
      : text}</span>;
  }
  return (
    <section className="conversation-event-card" aria-label="Conversation event">
      <div className="conversation-event-summary">
        {href ? <RouterLink className="conversation-event-title" to={href} state={coordinatorConversationLinkState(`${location.pathname}${location.search}${location.hash}`, href)}>{fragment('event-title', fragments.title)}</RouterLink>
          : remoteHref ? <a className="conversation-event-title" href={remoteHref}>{fragment('event-title', fragments.title)}</a>
            : fragment('event-title', fragments.title)}
        <span className={(available && (available.outcome.kind === 'failed' || available.outcome.kind === 'cleanup_failed')) || presentation?.status === 'federated_gap' || (presentation?.status === 'federated_fact' && presentation.outcome === 'failed') ? 'conversation-event-outcome failure' : 'conversation-event-outcome'}>{fragment('event-outcome', fragments.outcome)}</span>
        {timeUs !== null && <time dateTime={new Date(timeUs / 1000).toISOString()} aria-label={fragments.time}>{fragment('event-time', fragments.time)}</time>}
      </div>
      <details ref={details}>
        <summary>Event details</summary>
        {fragments.reason && <p className="conversation-event-reason">{fragment('event-reason', fragments.reason)}</p>}
        <pre className="conversation-event-provenance">{fragment('event-provenance', fragments.provenance)}</pre>
        <CopyButton text={fragments.provenance} title="Copy event provenance" />
        <CopyButton text={originalText} title="Copy original event text" />
        {children}
      </details>
    </section>
  );
}
