import { useCallback, useEffect, useRef, useState } from 'react';
import { api } from '../api';
import { refreshCodexQuota, resetOptions, type ResetOption } from '../codexResets';
import type { CodexResetCredits } from '../generated/CodexResetCredits';
import type { CodexResetRedeemResponse } from '../generated/CodexResetRedeemResponse';
import { generateUUID } from '../utils/uuid';
import './CodexResetFlow.css';

// `account` is the listing's account_id; the server refuses a redemption whose
// account no longer matches the signed-in credential.
interface Choices {
  options: ResetOption[];
  account: string | null;
}

type FlowState =
  | { type: 'loading' }
  | { type: 'unavailable'; notice: string | null }
  | { type: 'choosing'; choices: Choices; notice: string | null }
  | { type: 'confirming'; choices: Choices; option: ResetOption; key: string }
  | { type: 'in_flight'; choices: Choices; option: ResetOption; key: string }
  | { type: 'failed'; choices: Choices; option: ResetOption; key: string }
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
      setState({ type: 'unavailable', notice });
      return;
    }
    onListingRef.current?.(result.credits);
    const options = resetOptions(result.credits);
    if (options.length === 0) {
      setState({ type: 'done', text: notice ?? 'No usage limit resets are available.', succeeded: false });
      return;
    }
    setState({ type: 'choosing', choices: { options, account: result.credits.account_id }, notice });
  }, []);

  useEffect(() => { void loadChoices(null); }, [loadChoices]);

  const redeem = useCallback(async (choices: Choices, option: ResetOption, key: string) => {
    setState({ type: 'in_flight', choices, option, key });
    const result = await api.codexRedeemReset({
      idempotency_key: key,
      credit_id: option.creditId,
      account_id: choices.account,
    });
    if (result.type === 'ok' && result.response.outcome !== 'nothing_to_reset') {
      void refreshCodexQuota();
    }
    if (!mountedRef.current) return;
    switch (result.type) {
      case 'failed':
        setState({ type: 'failed', choices, option, key });
        return;
      case 'signed_out':
        setState({ type: 'done', text: 'Sign in to ChatGPT to use usage limit resets.', succeeded: false });
        return;
      case 'account_changed':
        void loadChoices('Your ChatGPT account changed. Choose a reset for the current account.');
        return;
      case 'ok':
        break;
      default:
        result satisfies never;
        return;
    }
    const response = result.response;
    switch (response.outcome) {
      case 'reset':
      case 'already_redeemed':
        setState({ type: 'done', text: 'Usage reset.', succeeded: true });
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
      {state.type === 'loading' && (
        <>
          <div className="codex-reset-flow__muted" role="status">Checking your resets…</div>
          <div className="codex-reset-flow__actions">
            <button type="button" className="codex-reset-flow__btn" onClick={onClose}>Cancel</button>
          </div>
        </>
      )}

      {state.type === 'unavailable' && (
        <>
          {state.notice && <div className="codex-reset-flow__notice" role="status">{state.notice}</div>}
          <div className="codex-reset-flow__muted" role="status">Couldn&apos;t load your resets.</div>
          <div className="codex-reset-flow__actions">
            <button type="button" className="codex-reset-flow__btn" onClick={() => { void loadChoices(state.notice); }}>Try again</button>
            <button type="button" className="codex-reset-flow__btn" onClick={onClose}>Close</button>
          </div>
        </>
      )}

      {state.type === 'choosing' && (
        <>
          {state.notice && <div className="codex-reset-flow__notice" role="status">{state.notice}</div>}
          <ul className="codex-reset-flow__options">
            {state.choices.options.map((option, index) => (
              <li key={option.creditId ?? `backend-choice-${index}`}>
                <button
                  type="button"
                  className="codex-reset-flow__option"
                  onClick={() => setState({ type: 'confirming', choices: state.choices, option, key: generateUUID() })}
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
            <div className="codex-reset-flow__error" role="status">Couldn&apos;t reset usage. Please try again.</div>
          )}
          <div className="codex-reset-flow__actions">
            <button
              type="button"
              className="codex-reset-flow__btn codex-reset-flow__btn--primary"
              disabled={state.type === 'in_flight'}
              onClick={() => { void redeem(state.choices, state.option, state.key); }}
            >
              {state.type === 'in_flight' ? 'Resetting…' : state.type === 'failed' ? 'Try again' : 'Yes, use reset'}
            </button>
            {state.type === 'confirming' && (
              <button
                type="button"
                className="codex-reset-flow__btn"
                onClick={() => setState({ type: 'choosing', choices: state.choices, notice: null })}
              >
                No, go back
              </button>
            )}
            {state.type !== 'in_flight' && (
              <button type="button" className="codex-reset-flow__btn" onClick={onClose}>
                {state.type === 'failed' ? 'Close' : 'Cancel'}
              </button>
            )}
          </div>
        </>
      )}

      {state.type === 'done' && (
        <>
          <div className={state.succeeded ? 'codex-reset-flow__success' : 'codex-reset-flow__muted'} role="status">
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
