import { useCallback, useEffect, useRef, useState } from 'react';
import { api, type CodexResetCreditsResult } from '../api';
import { resetCountLabel } from '../codexResets';
import { CodexResetFlow } from './CodexResetFlow';

interface CodexResetActionProps {
  /**
   * `error`: inline with a quota-exhaustion error; silent unless resets are
   * available (REQ-CRR-005). `settings`: always shows the count, and shows
   * failure distinctly from zero (REQ-CRR-009).
   */
  surface: 'error' | 'settings';
  /** Called after a successful redemption while this action is mounted. */
  onRedeemed?: (() => void) | undefined;
}

export function CodexResetAction({ surface, onRedeemed }: CodexResetActionProps) {
  const [listing, setListing] = useState<CodexResetCreditsResult | null>(null);
  const [open, setOpen] = useState(false);
  const mountedRef = useRef(true);
  const onRedeemedRef = useRef(onRedeemed);
  onRedeemedRef.current = onRedeemed;

  const load = useCallback(async () => {
    const result = await api.codexResetCredits();
    if (mountedRef.current) setListing(result);
  }, []);

  useEffect(() => {
    mountedRef.current = true;
    void load();
    return () => { mountedRef.current = false; };
  }, [load]);

  const handleSucceeded = useCallback(() => {
    void load();
    onRedeemedRef.current?.();
  }, [load]);

  const flow = open ? (
    <CodexResetFlow
      onSucceeded={handleSucceeded}
      onListing={(credits) => setListing({ type: 'ok', credits })}
      onClose={() => setOpen(false)}
    />
  ) : null;

  const count = listing?.type === 'ok' ? listing.credits.available_count : null;

  if (surface === 'error') {
    if (flow) return flow;
    if (count === null || count <= 0) return null;
    return (
      <div className="codex-reset-availability">
        <span>{resetCountLabel(count)}</span>
        <button type="button" className="codex-reset-availability__use" onClick={() => setOpen(true)}>
          Use reset
        </button>
      </div>
    );
  }

  if (listing === null || listing.type === 'signed_out') return null;
  return (
    <div className="codex-reset-settings">
      <div className="codex-reset-availability">
        {listing.type === 'unavailable' ? (
          <span className="codex-reset-availability__muted">Resets unavailable</span>
        ) : (
          <>
            <span>{count && count > 0 ? resetCountLabel(count) : 'No resets available'}</span>
            {count !== null && count > 0 && !open && (
              <button type="button" className="codex-reset-availability__use" onClick={() => setOpen(true)}>
                Use reset
              </button>
            )}
          </>
        )}
      </div>
      {flow}
    </div>
  );
}
