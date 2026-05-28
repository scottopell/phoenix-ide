import { useEffect, useState } from 'react';
import { usePhaseStateUpdatedAt } from '../conversation/useConversationAtom';

/**
 * Pre-first-byte placeholder rendered as the synthetic `pending_agent`
 * tail unit by `renderUnits.ts` (REQ-WPV-006). Occupies the same DOM
 * slot the `streaming_agent` bubble will use once the first token
 * arrives, so the visual transition is contents-only (no DOM remount,
 * no scroll jump).
 *
 * Reads `phaseStateUpdatedAt` from the conversation atom and ticks an
 * elapsed-seconds counter at 1 Hz — server-authoritative timestamp
 * means the counter survives reconnect/reload and stays consistent
 * across tabs viewing the same conversation.
 */
export function PendingAssistantBubble({ slug }: { slug: string }) {
  const phaseStateUpdatedAt = usePhaseStateUpdatedAt(slug);

  const [elapsedSeconds, setElapsedSeconds] = useState(0);
  useEffect(() => {
    if (phaseStateUpdatedAt == null) {
      setElapsedSeconds(0);
      return;
    }
    const compute = () =>
      setElapsedSeconds(Math.max(0, Math.floor((Date.now() - phaseStateUpdatedAt) / 1000)));
    compute();
    const interval = window.setInterval(compute, 1000);
    return () => window.clearInterval(interval);
  }, [phaseStateUpdatedAt]);

  return (
    <div className="message-row agent">
      <div className="message-content pending-assistant-bubble">
        <span className="pending-assistant-bubble__label">thinking</span>
        {phaseStateUpdatedAt != null && (
          <span className="pending-assistant-bubble__elapsed"> {elapsedSeconds}s</span>
        )}
      </div>
    </div>
  );
}
