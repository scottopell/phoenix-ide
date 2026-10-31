import { useCallback, useEffect, useRef, useState } from 'react';
import { api } from '../api';
import { refreshCodexQuota, resetOptions, type ResetOption } from '../codexResets';
import type { CodexResetCredits } from '../generated/CodexResetCredits';
import type { CodexResetRedeemResponse } from '../generated/CodexResetRedeemResponse';
import { generateUUID } from '../utils/uuid';
import './CodexResetFlow.css';

// Choose → confirm → redeem (specs/codex-rate-limit-resets REQ-CRR-006/007).
// The idempotency key is minted when confirmation opens and reused by
// "Try again", so a lost success answers already_redeemed instead of spending
// a second credit.
type FlowState =
  | { type: 'loading' }
  | { type: 'unavailable' }
  | { type: 'choosing'; options: ResetOption[]; notice: string | null }
  | { type: 'confirming'; options: ResetOption[]; option: ResetOption; key: string }
  | { type: 'in_flight'; options: ResetOption[]; option: ResetOption; key: string }
  | { type: 'failed'; options: ResetOption[]; option: ResetOption; key: string }
  | { type: 'done'; text: string; succeeded: boolean };

interface CodexResetFlowProps {
  /** Called once per successful redemption, only while this flow is mounted. */
  onSucceeded?: ((response: CodexResetRedeemResponse) => void) | undefined;
  /** Called with every fresh listing the flow loads. */
  onListing?: ((listing: CodexResetCredits) => void) | undefined;
  onClose: () => void;
}

export function CodexResetFlow({ onSucceeded, onListing, onClose }: CodexResetFlowProps) {
  const [state, setState] = useState<FlowState>({ type: 'loading' });
  const mountedRef = useRef(true);
  const onSucceededRef = useRef(onSucceeded);
  const onListingRef = useRef(onListing);
  onSucceededRef.current = onSucceeded;
  onListingRef.current = onListing;

  useEffect(() => {
    mountedRef.current = true;
    return () => { mountedRef.current = false; };
  }, []);

  const loadChoices = useCallback(async (notice: string | null) => {
    setState({ type: 'loading' });
    const result = await api.codexResetCredits();
    if (!mountedRef.current) return;
    if (result.type !== 'ok') {
      setState({ type: 'unavailable' });
      return;
    }
    onListingRef.current?.(result.credits);
    const options = resetOptions(result.credits);
    if (options.length === 0) {
      setState({ type: 'done', text: notice ?? 'No usage limit resets are available.', succeeded: false });
      return;
    }
    setState({ type: 'choosing', options, notice });
  }, []);

  useEffect(() => { void loadChoices(null); }, [loadChoices]);

  const redeem = useCallback(async (options: ResetOption[], option: ResetOption, key: string) => {
    setState({ type: 'in_flight', options, option, key });
    let response: CodexResetRedeemResponse;
    try {
      response = await api.codexRedeemReset({ idempotency_key: key, credit_id: option.creditId });
    } catch {
      if (mountedRef.current) setState({ type: 'failed', options, option, key });
      return;
    }
    if (!mountedRef.current) return;
    switch (response.outcome) {
      case 'reset':
      case 'already_redeemed':
        setState({ type: 'done', text: 'Usage reset.', succeeded: true });
        void refreshCodexQuota();
        onSucceededRef.current?.(response);
        return;
      case 'nothing_to_reset':
        setState({
          type: 'done',
          text: 'Your usage does not need a reset right now. No reset was used.',
          succeeded: false,
        });
        return;
      case 'no_credit':
        if (option.creditId !== null) {
          void loadChoices('That reset is no longer available.');
        } else {
          setState({ type: 'done', text: 'No usage limit resets are available.', succeeded: false });
        }
        return;
      default:
        response.outcome satisfies never;
    }
  }, [loadChoices]);

  return (
    <div className="codex-reset-flow" role="group" aria-label="Usage limit resets">
      {state.type === 'loading' && <div className="codex-reset-flow__muted">Checking your resets…</div>}

      {state.type === 'unavailable' && (
        <>
          <div className="codex-reset-flow__muted">Couldn&apos;t load your resets.</div>
          <div className="codex-reset-flow__actions">
            <button type="button" className="codex-reset-flow__btn" onClick={() => { void loadChoices(null); }}>Try again</button>
            <button type="button" className="codex-reset-flow__btn" onClick={onClose}>Close</button>
          </div>
        </>
      )}

      {state.type === 'choosing' && (
        <>
          {state.notice && <div className="codex-reset-flow__notice">{state.notice}</div>}
          <ul className="codex-reset-flow__options">
            {state.options.map((option, index) => (
              <li key={option.creditId ?? `backend-choice-${index}`}>
                <button
                  type="button"
                  className="codex-reset-flow__option"
                  onClick={() => setState({ type: 'confirming', options: state.options, option, key: generateUUID() })}
                >
                  <span className="codex-reset-flow__option-name">{option.name}</span>
                  <span className="codex-reset-flow__muted">{option.detail ?? option.description}</span>
                </button>
              </li>
            ))}
          </ul>
          <div className="codex-reset-flow__actions">
            <button type="button" className="codex-reset-flow__btn" onClick={onClose}>Cancel</button>
          </div>
        </>
      )}

      {(state.type === 'confirming' || state.type === 'in_flight' || state.type === 'failed') && (
        <>
          <div className="codex-reset-flow__title">Use this reset?</div>
          <div>
            <span className="codex-reset-flow__option-name">{state.option.name}</span>
            {state.option.detail && <span className="codex-reset-flow__muted"> · {state.option.detail}</span>}
          </div>
          <div className="codex-reset-flow__muted">{state.option.description}</div>
          {state.type === 'failed' && (
            <div className="codex-reset-flow__error">Couldn&apos;t reset usage. Please try again.</div>
          )}
          <div className="codex-reset-flow__actions">
            <button
              type="button"
              className="codex-reset-flow__btn codex-reset-flow__btn--primary"
              disabled={state.type === 'in_flight'}
              onClick={() => { void redeem(state.options, state.option, state.key); }}
            >
              {state.type === 'in_flight' ? 'Resetting…' : state.type === 'failed' ? 'Try again' : 'Yes, use reset'}
            </button>
            <button
              type="button"
              className="codex-reset-flow__btn"
              disabled={state.type === 'in_flight'}
              onClick={() => setState({ type: 'choosing', options: state.options, notice: null })}
            >
              No, go back
            </button>
          </div>
        </>
      )}

      {state.type === 'done' && (
        <>
          <div className={state.succeeded ? 'codex-reset-flow__success' : 'codex-reset-flow__muted'}>
            {state.succeeded ? '✓ ' : ''}{state.text}
          </div>
          <div className="codex-reset-flow__actions">
            <button type="button" className="codex-reset-flow__btn" onClick={onClose}>Close</button>
          </div>
        </>
      )}
    </div>
  );
}
