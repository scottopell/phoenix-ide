import { useCallback, useEffect, useRef, useState } from 'react';
import { api, type CodexLoginPreflight } from '../api';
import { refreshModels } from '../modelsPoller';

interface AccountChipProps {
  preflight: CodexLoginPreflight | null;
  onPreflightInvalidated: () => void;
  /** Tight rendering for the collapsed sidebar (48px wide). Shows a single
   *  letter so the chip fits alongside the icon-strip toggles. */
  compact?: boolean;
}

function shortAccount(id: string | null): string {
  if (!id) return 'unknown';
  if (id.length <= 12) return id;
  return `${id.slice(0, 4)}…${id.slice(-4)}`;
}

export function AccountChip({ preflight, onPreflightInvalidated, compact = false }: AccountChipProps) {
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const wrapRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!open) return undefined;
    const onDocClick = (e: MouseEvent) => {
      if (!wrapRef.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false);
    };
    document.addEventListener('mousedown', onDocClick);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDocClick);
      document.removeEventListener('keydown', onKey);
    };
  }, [open]);

  const handleSignOut = useCallback(async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await api.codexSignout();
      await refreshModels();
      onPreflightInvalidated();
      setOpen(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }, [busy, onPreflightInvalidated]);

  if (!preflight?.already_signed_in) return null;

  // Prefer email over the opaque chatgpt_account_id UUID for human-readable
  // identity. Fall back to the short id only when the id_token didn't carry
  // an `email` claim (older tokens, or scopes that omitted the email scope).
  const identity = preflight.account_email ?? (preflight.account_id ? shortAccount(preflight.account_id) : null);
  const tooltip = identity
    ? `Signed in as ${identity}`
    : 'Signed in to Codex';

  return (
    <div className="account-chip-wrap" ref={wrapRef}>
      <button
        type="button"
        className={`account-chip${compact ? ' account-chip--compact' : ''}`}
        title={tooltip}
        aria-label={tooltip}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
      >
        {compact ? 'C' : 'Codex'}
      </button>
      {open && (
        <div className="account-chip-menu" role="menu">
          <div className="account-chip-menu-header">
            <div className="account-chip-menu-label">Signed in as</div>
            {identity && (
              <div
                className="account-chip-menu-id"
                title={preflight.account_id ?? undefined}
              >
                {preflight.account_email ? identity : <code>{identity}</code>}
              </div>
            )}
          </div>
          {error && <div className="account-chip-menu-error">{error}</div>}
          <button
            type="button"
            className="account-chip-menu-item"
            onClick={() => { void handleSignOut(); }}
            disabled={busy}
            role="menuitem"
          >
            {busy ? 'Signing out…' : 'Sign out'}
          </button>
        </div>
      )}
    </div>
  );
}
