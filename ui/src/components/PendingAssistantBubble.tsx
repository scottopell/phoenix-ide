import { useEffect, useState } from 'react';
import { usePhase, usePhaseStateUpdatedAt } from '../conversation/useConversationAtom';
import { getStateDescription } from '../utils';

/**
 * Pre-first-byte placeholder rendered as the synthetic `pending_agent`
 * tail unit by `renderUnits.ts` (REQ-WPV-006). Occupies the same DOM
 * slot the `streaming_agent` bubble will use once the first token
 * arrives, so the visual transition is contents-only (no DOM remount,
 * no scroll jump).
 *
 * Label comes from `getStateDescription(phase)` — the same helper the
 * StateBar uses — so the bubble's prose stays in sync with whatever
 * the StateBar shows for the same phase (`awaiting LLM response` for
 * `llm_requesting`, `preparing request` for `awaiting_llm`, `starting`
 * for `seeded_llm_requesting`). Hardcoded prose was the prior shape;
 * it diverged from the StateBar on the non-`llm_requesting` variants
 * and defeated the spec's "UI reflects current state" goal.
 *
 * Elapsed counter ticks at 1 Hz, derived from `phaseStateUpdatedAt`
 * (server-authoritative — survives reconnect, reload, multi-tab).
 */
export function PendingAssistantBubble({ slug }: { slug: string }) {
  const phaseStateUpdatedAt = usePhaseStateUpdatedAt(slug);
  const phase = usePhase(slug);

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

  // Strip the trailing `...` that `getStateDescription` appends to
  // working-phase prose — the elapsed counter is the "we're still
  // waiting" affordance, so the ellipsis would double up. e.g.
  // `"awaiting LLM response..."` becomes `"awaiting LLM response"`, then we render
  // `"awaiting LLM response 4s"`.
  const label = getStateDescription(phase).replace(/\.{3}$/, '');

  return (
    <div className="message-row agent">
      <div className="message-content pending-assistant-bubble">
        <span className="pending-assistant-bubble__label">{label}</span>
        {phaseStateUpdatedAt != null && (
          <span className="pending-assistant-bubble__elapsed"> {elapsedSeconds}s</span>
        )}
      </div>
    </div>
  );
}
