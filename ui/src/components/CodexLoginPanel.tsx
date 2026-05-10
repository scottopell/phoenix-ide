import { useCallback, useEffect, useRef, useState } from 'react';
import {
  api,
  type CodexDeviceStartResponse,
  type CodexLoginPreflight,
  type CodexLoginStatus,
  type CodexPkceStartResponse,
} from '../api';
import { refreshModels } from '../modelsPoller';

type Mode = 'choose' | 'pkce' | 'device';

type FlowResult = { kind: 'success'; accountId: string | null; authPath: string };

const POLL_INTERVAL_MS = 1500;

interface CodexLoginPanelProps {
  onDismiss?: () => void;
}

export function CodexLoginPanel({ onDismiss }: CodexLoginPanelProps) {
  const [preflight, setPreflight] = useState<CodexLoginPreflight | null>(null);
  const [mode, setMode] = useState<Mode>('choose');
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<FlowResult | null>(null);
  const [pkceStarting, setPkceStarting] = useState(false);
  const [pkceStart, setPkceStart] = useState<CodexPkceStartResponse | null>(null);

  useEffect(() => {
    let cancelled = false;
    api.codexLoginPreflight()
      .then((p) => { if (!cancelled) setPreflight(p); })
      .catch((e) => { if (!cancelled) setError(e instanceof Error ? e.message : String(e)); });
    return () => { cancelled = true; };
  }, []);

  const handleSuccess = useCallback((r: FlowResult) => {
    setResult(r);
    setMode('choose');
    setPkceStart(null);
    setError(null);
    void refreshModels().catch(() => {});
    api.codexLoginPreflight()
      .then((p) => setPreflight(p))
      .catch(() => {});
  }, []);

  // Single-click browser sign-in: open a popup synchronously inside the user
  // gesture (popup blockers require this), kick off pkce/start in parallel,
  // then redirect the popup to the authorize URL once the response is back.
  // Skipping the synchronous popup and relying on a post-await window.open
  // gets blocked by Safari and Firefox.
  const handlePickPkce = useCallback(() => {
    setError(null);
    if (pkceStarting) return;
    setPkceStarting(true);
    const popup = window.open('about:blank', 'codex-signin');
    api.codexPkceStart()
      .then((s) => {
        setPkceStart(s);
        setMode('pkce');
        if (popup && !popup.closed) {
          popup.location.href = s.authorize_url;
        }
        setPkceStarting(false);
      })
      .catch((e) => {
        if (popup && !popup.closed) popup.close();
        setError(e instanceof Error ? e.message : String(e));
        setPkceStarting(false);
      });
  }, [pkceStarting]);

  if (result) {
    return (
      <div className="codex-login-panel">
        <SuccessBanner
          preflight={preflight}
          {...(onDismiss !== undefined ? { onDismiss } : {})}
        />
      </div>
    );
  }

  return (
    <div className="codex-login-panel">
      {error && <div className="login-error">{error}</div>}

      {mode === 'choose' && (
        <ChooseFlow
          preflight={preflight}
          onPickPkce={handlePickPkce}
          onPickDevice={() => { setError(null); setMode('device'); }}
          pkceStarting={pkceStarting}
          {...(onDismiss !== undefined ? { onCancel: onDismiss } : {})}
        />
      )}

      {mode === 'pkce' && pkceStart && (
        <PkceFlow
          start={pkceStart}
          onCancel={() => { setPkceStart(null); setMode('choose'); }}
          onSuccess={handleSuccess}
          onError={(msg) => { setError(msg); setPkceStart(null); setMode('choose'); }}
        />
      )}

      {mode === 'device' && (
        <DeviceFlow
          onCancel={() => setMode('choose')}
          onSuccess={handleSuccess}
          onError={(msg) => { setError(msg); setMode('choose'); }}
        />
      )}
    </div>
  );
}

function SuccessBanner({
  preflight,
  onDismiss,
}: {
  preflight: CodexLoginPreflight | null;
  onDismiss?: () => void;
}) {
  return (
    <div className="codex-login-success">
      <div className="codex-login-success-title">You&rsquo;re signed in</div>
      <div className="codex-login-success-body">
        Codex bridge is active &mdash; your next message will use your ChatGPT subscription.
        {preflight?.restart_required_after_login && (
          <div className="codex-login-warning">
            Phoenix couldn&rsquo;t hot-reload the credential. Restart Phoenix
            and the bridge will pick up your new login on the next boot.
          </div>
        )}
      </div>
      {onDismiss && (
        <button
          type="button"
          className="login-button codex-login-success-cta"
          onClick={onDismiss}
        >
          Get started &rarr;
        </button>
      )}
    </div>
  );
}

function ChooseFlow({
  preflight,
  onPickPkce,
  onPickDevice,
  pkceStarting,
  onCancel,
}: {
  preflight: CodexLoginPreflight | null;
  onPickPkce: () => void;
  onPickDevice: () => void;
  pkceStarting: boolean;
  onCancel?: () => void;
}) {
  return (
    <div className="codex-login-choose">
      {preflight?.already_signed_in && (
        <div className="codex-login-info">
          You appear to be signed in already (tokens at <code>{preflight.auth_path}</code>).
          Signing in again replaces them.
        </div>
      )}
      <button
        type="button"
        className="login-button"
        onClick={onPickPkce}
        disabled={pkceStarting}
      >
        {pkceStarting ? 'Opening sign-in…' : 'Sign in with browser'}
      </button>
      <button type="button" className="login-button login-button-secondary" onClick={onPickDevice}>
        Sign in with a code (no browser on this host)
      </button>
      <p className="codex-login-help">
        The browser flow opens auth.openai.com and redirects to a local port (1455).
        Use the code flow on SSH/headless hosts where that won&rsquo;t work.
      </p>
      {onCancel && (
        <button type="button" className="codex-login-cancel" onClick={onCancel}>
          Cancel
        </button>
      )}
    </div>
  );
}

function PkceFlow({
  start,
  onCancel,
  onSuccess,
  onError,
}: {
  start: CodexPkceStartResponse;
  onCancel: () => void;
  onSuccess: (r: FlowResult) => void;
  onError: (msg: string) => void;
}) {
  const [pasteUrl, setPasteUrl] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [showPaste, setShowPaste] = useState(!start.loopback_bound);

  useEffect(() => {
    let cancelled = false;
    let timer: number | undefined;
    const tick = async () => {
      try {
        const status = await api.codexPkceStatus(start.session_id);
        if (cancelled) return;
        if (status.kind === 'success') {
          onSuccess({ kind: 'success', accountId: status.account_id, authPath: status.auth_path });
          return;
        }
        if (status.kind === 'error') {
          onError(status.message);
          return;
        }
      } catch (e) {
        if (cancelled) return;
        onError(e instanceof Error ? e.message : String(e));
        return;
      }
      if (!cancelled) timer = window.setTimeout(tick, POLL_INTERVAL_MS);
    };
    timer = window.setTimeout(tick, POLL_INTERVAL_MS);
    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [start, onSuccess, onError]);

  const handleCancel = useCallback(() => {
    void api.codexPkceCancel(start.session_id);
    onCancel();
  }, [start, onCancel]);

  const handlePaste = useCallback(async () => {
    setSubmitting(true);
    try {
      await api.codexPkceManual(start.session_id, { redirect_url: pasteUrl.trim() });
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e));
    } finally {
      setSubmitting(false);
    }
  }, [start, pasteUrl, onError]);

  return (
    <div className="codex-login-flow">
      {!start.loopback_bound && (
        <div className="codex-login-warning">
          Couldn&rsquo;t bind localhost:{start.callback_port}; browser callback won&rsquo;t reach Phoenix.
          Use the manual paste below.
        </div>
      )}

      <div className="codex-login-status">
        Waiting for sign-in in your browser&hellip;
      </div>
      <p className="codex-login-help" style={{ textAlign: 'center' }}>
        Didn&rsquo;t see a tab open?{' '}
        <a href={start.authorize_url} target="_blank" rel="noopener noreferrer">
          Open it manually
        </a>
        .
      </p>

      <button
        type="button"
        className="codex-login-paste-toggle"
        onClick={() => setShowPaste((v) => !v)}
      >
        {showPaste ? 'Hide manual paste' : 'Browser callback didn\'t fire? Paste the redirect URL'}
      </button>

      {showPaste && (
        <div className="codex-login-paste">
          <p className="codex-login-help">
            After signing in, your browser is redirected to a URL starting with{' '}
            <code>{start.redirect_uri}?code=…&amp;state=…</code>.
            Copy the full URL from your address bar and paste it here.
          </p>
          <input
            type="text"
            className="login-input"
            placeholder="Paste full redirect URL"
            value={pasteUrl}
            onChange={(e) => setPasteUrl(e.target.value)}
            disabled={submitting}
          />
          <button
            type="button"
            className="login-button"
            onClick={handlePaste}
            disabled={submitting || pasteUrl.trim().length === 0}
          >
            {submitting ? 'Submitting…' : 'Submit'}
          </button>
        </div>
      )}

      <button type="button" className="codex-login-cancel" onClick={handleCancel}>
        Cancel
      </button>
    </div>
  );
}

function DeviceFlow({
  onCancel,
  onSuccess,
  onError,
}: {
  onCancel: () => void;
  onSuccess: (r: FlowResult) => void;
  onError: (msg: string) => void;
}) {
  const [start, setStart] = useState<CodexDeviceStartResponse | null>(null);
  const onErrorRef = useRef(onError);
  useEffect(() => { onErrorRef.current = onError; });

  useEffect(() => {
    let sessionId: string | null = null;
    const timer = window.setTimeout(() => {
      api.codexDeviceStart()
        .then((s) => {
          sessionId = s.session_id;
          setStart(s);
        })
        .catch((e) => { onErrorRef.current(e instanceof Error ? e.message : String(e)); });
    }, 0);
    return () => {
      window.clearTimeout(timer);
      if (sessionId !== null) void api.codexDeviceCancel(sessionId);
    };
  }, []);

  useEffect(() => {
    if (!start) return undefined;
    let cancelled = false;
    let timer: number | undefined;
    const intervalMs = Math.max(1000, start.interval_secs * 1000);
    const tick = async () => {
      try {
        const status: CodexLoginStatus = await api.codexDeviceStatus(start.session_id);
        if (cancelled) return;
        if (status.kind === 'success') {
          onSuccess({ kind: 'success', accountId: status.account_id, authPath: status.auth_path });
          return;
        }
        if (status.kind === 'error') {
          onError(status.message);
          return;
        }
      } catch (e) {
        if (cancelled) return;
        onError(e instanceof Error ? e.message : String(e));
        return;
      }
      if (!cancelled) timer = window.setTimeout(tick, intervalMs);
    };
    timer = window.setTimeout(tick, intervalMs);
    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [start, onSuccess, onError]);

  const handleCancel = useCallback(() => {
    if (start) void api.codexDeviceCancel(start.session_id);
    onCancel();
  }, [start, onCancel]);

  if (!start) {
    return <div className="codex-login-info">Requesting device code&hellip;</div>;
  }

  return (
    <div className="codex-login-flow">
      <ol className="codex-login-steps">
        <li>
          Open this URL on any device:
          <div className="codex-login-link">
            <a href={start.verification_url} target="_blank" rel="noopener noreferrer">
              {start.verification_url}
            </a>
          </div>
        </li>
        <li>
          Enter this code:
          <div className="codex-login-code">{start.user_code}</div>
        </li>
        <li>
          Sign in with your OpenAI account. Phoenix will detect completion automatically.
        </li>
      </ol>
      <div className="codex-login-status">
        Waiting (expires in {Math.floor(start.timeout_secs / 60)} minutes)&hellip;
      </div>
      <button type="button" className="codex-login-cancel" onClick={handleCancel}>
        Cancel
      </button>
    </div>
  );
}
