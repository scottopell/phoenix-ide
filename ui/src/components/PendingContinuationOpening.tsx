import { useEffect, useRef, useState } from 'react';
import { api } from '../api';

export function PendingContinuationOpening({ predecessorId, enabled, onRecovered }: {
  predecessorId: string;
  enabled: boolean;
  onRecovered: () => void;
}) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const active = useRef(false);
  const inFlight = useRef(false);
  useEffect(() => {
    active.current = true;
    return () => { active.current = false; };
  }, []);

  const retry = async () => {
    if (!enabled || inFlight.current) return;
    inFlight.current = true;
    setPending(true);
    setError(null);
    try {
      const result = await api.retryContinuationOpening(predecessorId);
      if (!active.current) return;
      if (result.status === 'dispatch_failed') {
        setError(result.error ?? 'Continuation opening could not be dispatched. Retry the saved handoff.');
      } else {
        onRecovered();
      }
    } catch (cause) {
      if (active.current) setError(cause instanceof Error ? cause.message : 'Could not retry the saved handoff.');
    } finally {
      inFlight.current = false;
      if (active.current) setPending(false);
    }
  };

  return <div role="status" aria-label="Continuation opening pending">
    <span>Continuation opening pending. The saved handoff has not been accepted.</span>{' '}
    <button type="button" disabled={!enabled || pending} onClick={() => void retry()}>
      {pending ? 'Retrying saved handoff…' : 'Retry saved handoff'}
    </button>
    {error && <div role="alert">{error}</div>}
  </div>;
}
