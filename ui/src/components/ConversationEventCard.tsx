import { useLayoutEffect, useRef, type MouseEvent, type ReactNode } from 'react';
import { Link as RouterLink, useLocation } from 'react-router-dom';
import type { EventPresentation } from '../generated/EventPresentation';
import type { ConversationHighlight } from './MessageComponents';
import { CopyButton } from './CopyButton';
import { coordinatorConversationLinkState } from './conversationReturnOrigin';
import {
  FEDERATION_CONNECTIONS_SETTINGS_HREF,
  requestOpenFederationConnections,
} from './settingsEvents';
import './ConversationEventCard.css';

// eslint-disable-next-line react-refresh/only-export-components
export function eventCardFragments(presentation: EventPresentation | null, eventId: string) {
  if (!presentation || presentation.status === 'unavailable') {
    return { title: 'Conversation event', outcome: 'Details unavailable', time: '', provenance: `Event ID: ${presentation?.event_id ?? eventId}${presentation ? `\nUnavailable: ${presentation.reason}` : ''}`, reason: '' };
  }
  if (presentation.status === 'federated_fact' || presentation.status === 'federated_gap') {
    const labels = { completed: 'Turn finished', failed: 'Failed', cancelled: 'Cancelled' };
    const isFact = presentation.status === 'federated_fact';
    const current = !isFact ? presentation.current : null;
    return {
      title: `Remote conversation · ${presentation.instance_id.slice(0, 8)}`,
      outcome: isFact ? labels[presentation.outcome] : 'Retention gap',
      time: isFact ? new Date(presentation.occurred_at_us / 1000).toLocaleString() : '',
      reason: isFact ? 'Terminal execution fact; task completion is not inferred.'
        : `Intermediate facts unavailable. Current state: ${current?.kind === 'terminal' ? labels[current.outcome] : 'Nonterminal'}.`,
      provenance: [
        `Event ID: ${presentation.event_id}`, `Instance ID: ${presentation.instance_id}`,
        `Conversation ID: ${presentation.product_conversation_id}`,
        `Transcript ID: ${isFact ? presentation.source_transcript_id : presentation.current_transcript_id}`,
        isFact ? `Time (microseconds): ${presentation.occurred_at_us}`
          : `Gap after: ${presentation.requested_after}\nResume after: ${presentation.resume_after}`,
        `Navigation: ${presentation.navigation.status === 'unavailable' ? presentation.navigation.reason : 'Destination HTTPS'}`,
      ].join('\n'),
    };
  }
  const outcome = presentation.outcome;
  const labels = {
    turn_finished: 'Turn finished', failed: 'Failed', cleanup_failed: 'Cleanup failed',
    cancelled: 'Cancelled', awaiting_user_response: 'Awaiting user response', awaiting_task_approval: 'Awaiting task approval',
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
  const remote = presentation?.status === 'federated_fact' || presentation?.status === 'federated_gap' ? presentation : null;
  const remoteNavigation = remote?.navigation.status === 'available' ? remote.navigation : null;
  const occurredAt = available?.occurred_at_us ?? (remote?.status === 'federated_fact' ? remote.occurred_at_us : null);
  const failure = available ? available.outcome.kind === 'failed' || available.outcome.kind === 'cleanup_failed'
    : remote?.status === 'federated_fact' && remote.outcome === 'failed';
  const href = available ? `/c/${encodeURIComponent(available.product_conversation_id)}` : null;
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
  function openConnections(event: MouseEvent<HTMLAnchorElement>) {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    if (requestOpenFederationConnections()) event.preventDefault();
  }
  return (
    <section className="conversation-event-card" aria-label="Conversation event">
      <div className="conversation-event-summary">
        {href ? <RouterLink className="conversation-event-title" to={href} state={coordinatorConversationLinkState(`${location.pathname}${location.search}${location.hash}`, href)}>{fragment('event-title', fragments.title)}</RouterLink>
          : remoteNavigation ? <a className="conversation-event-title" href={remoteNavigation.stable_url}>{fragment('event-title', fragments.title)}</a>
            : fragment('event-title', fragments.title)}
        <span className={failure ? 'conversation-event-outcome failure' : 'conversation-event-outcome'}>{fragment('event-outcome', fragments.outcome)}</span>
        {occurredAt !== null && <time dateTime={new Date(occurredAt / 1000).toISOString()} aria-label={fragments.time}>{fragment('event-time', fragments.time)}</time>}
      </div>
      {remoteNavigation && <p className="conversation-event-navigation-guidance">
        If this configured remote destination is unreachable, verify its saved URL in{' '}
        <RouterLink className="conversation-event-navigation-action" to={FEDERATION_CONNECTIONS_SETTINGS_HREF} onClick={openConnections}>Settings → Connections</RouterLink> before retrying.
      </p>}
      {remote?.navigation.status === 'unavailable' && remote.navigation.reason === 'missing_connection'
        && <p className="conversation-event-navigation-guidance">Remote navigation unavailable: no connection is configured.{' '}
          <RouterLink className="conversation-event-navigation-action" to={FEDERATION_CONNECTIONS_SETTINGS_HREF} onClick={openConnections}>Open Settings → Connections</RouterLink> to add one.
        </p>}
      <details ref={details}>
        <summary>Event details</summary>
        {remoteNavigation && <a href={remoteNavigation.source_url}>Exact destination transcript</a>}
        {remote?.navigation.status === 'unavailable' && remote.navigation.reason !== 'missing_connection'
          && <p>Remote navigation unavailable: {remote.navigation.reason.replaceAll('_', ' ')}</p>}
        {fragments.reason && <p className="conversation-event-reason">{fragment('event-reason', fragments.reason)}</p>}
        <pre className="conversation-event-provenance">{fragment('event-provenance', fragments.provenance)}</pre>
        <CopyButton text={fragments.provenance} title="Copy event provenance" />
        <CopyButton text={originalText} title="Copy original event text" />
        {children}
      </details>
    </section>
  );
}
