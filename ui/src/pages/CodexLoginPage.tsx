import { useCallback, useEffect, useRef, useState } from 'react';
import {
  api,
  type CodexDeviceStartResponse,
  type CodexLoginPreflight,
  type CodexLoginStatus,
  type CodexPkceStartResponse,
} from '../api';

type Mode = 'choose' | 'pkce' | 'device';

type FlowResult = { kind: 'success'; accountId: string | null; authPath: string };

const POLL_INTERVAL_MS = 1500;

export function CodexLoginPage() {
  const [preflight, setPreflight] = useState<CodexLoginPreflight | null>(null);
  const [mode, setMode] = useState<Mode>('choose');
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<FlowResult | null>(null);
  // PKCE-specific: a tab pre-opened from the user's click on "Sign in with
  // browser". Browsers require window.open() to fire inside a synchronous
  // user-gesture handler, so we open `about:blank` immediately and rewrite
  // its location once the authorize URL arrives from the API. Held in a ref
  // (not state) because changing it shouldn't trigger a re-render.
  const pkcePreopenedRef = useRef<Window | null>(null);

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
  }, []);

  return (
    <div className="login-page">
      <div className="login-card codex-login-card">
        <div className="login-header">
          <img src="/phoenix.svg" alt="Phoenix" className="login-logo" />
          <h1 className="login-title">Sign in with ChatGPT</h1>
          <p className="codex-login-subtitle">
            Use your ChatGPT Plus, Pro, Team, or Enterprise subscription with Phoenix.
          </p>
        </div>

        {result && <SuccessBanner result={result} preflight={preflight} />}
        {error && <div className="login-error">{error}</div>}

        {mode === 'choose' && (
          <ChooseFlow
            preflight={preflight}
            onPickPkce={() => {
              setError(null);
              // Open the destination tab in the user-gesture window. We only
              // know the URL after the /pkce/start round-trip completes, so
              // navigate this blank tab from the API success handler below.
              //
              // No `noopener` — when honored it makes window.open return
              // `null`, which would leave us with a blank tab we can't
              // navigate. We need the handle here, and the destination is
              // our own intent (auth.openai.com), not arbitrary content.
              pkcePreopenedRef.current = window.open('about:blank', '_blank');
              setMode('pkce');
            }}
            onPickDevice={() => { setError(null); setMode('device'); }}
          />
        )}

        {mode === 'pkce' && (
          <PkceFlow
            preopenedWindow={pkcePreopenedRef.current}
            onCancel={() => {
              if (pkcePreopenedRef.current) {
                pkcePreopenedRef.current.close();
                pkcePreopenedRef.current = null;
              }
              setMode('choose');
            }}
            onSuccess={handleSuccess}
            onError={(msg) => {
              // Close the popup on the error path too — otherwise a /pkce/start
              // failure or polling error leaves a blank tab open.
              if (pkcePreopenedRef.current) {
                pkcePreopenedRef.current.close();
                pkcePreopenedRef.current = null;
              }
              setError(msg);
              setMode('choose');
            }}
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
    </div>
  );
}

function SuccessBanner({ result, preflight }: { result: FlowResult; preflight: CodexLoginPreflight | null }) {
  return (
    <div className="codex-login-success">
      <div className="codex-login-success-title">Signed in</div>
      <div className="codex-login-success-body">
        Tokens written to <code>{result.authPath}</code>
        {result.accountId && <> for account <code>{result.accountId}</code></>}.
        {preflight && !preflight.bridge_loaded_at_startup && (
          <div className="codex-login-warning">
            <strong>Restart Phoenix</strong> to start using your ChatGPT subscription.
            (The credential is loaded at startup; we can't pick it up mid-session yet.)
          </div>
        )}
      </div>
    </div>
  );
}

function ChooseFlow({
  preflight,
  onPickPkce,
  onPickDevice,
}: {
  preflight: CodexLoginPreflight | null;
  onPickPkce: () => void;
  onPickDevice: () => void;
}) {
  return (
    <div className="codex-login-choose">
      {preflight?.already_signed_in && (
        <div className="codex-login-info">
          You appear to be signed in already (tokens at <code>{preflight.auth_path}</code>).
          Signing in again replaces them.
        </div>
      )}
      <button type="button" className="login-button" onClick={onPickPkce}>
        Sign in with browser
      </button>
      <button type="button" className="login-button login-button-secondary" onClick={onPickDevice}>
        Sign in with a code (no browser on this host)
      </button>
      <p className="codex-login-help">
        The browser flow opens auth.openai.com and redirects to a local port (1455).
        Use the code flow on SSH/headless hosts where that won't work.
      </p>
    </div>
  );
}

function PkceFlow({
  preopenedWindow,
  onCancel,
  onSuccess,
  onError,
}: {
  preopenedWindow: Window | null;
  onCancel: () => void;
  onSuccess: (r: FlowResult) => void;
  onError: (msg: string) => void;
}) {
  const [start, setStart] = useState<CodexPkceStartResponse | null>(null);
  const [pasteCode, setPasteCode] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [showPaste, setShowPaste] = useState(false);
  const startedRef = useRef(false);

  // Kick off the flow exactly once.
  useEffect(() => {
    if (startedRef.current) return;
    startedRef.current = true;
    let cancelled = false;
    api.codexPkceStart()
      .then((s) => {
        if (cancelled) return;
        setStart(s);
        // If loopback bind failed we MUST surface the manual paste path
        // immediately — the browser callback will go nowhere.
        if (!s.loopback_bound) setShowPaste(true);
        // Navigate the tab the user opened on click into the authorize URL.
        // If the popup was blocked or the user closed it, fall back to a
        // user-clickable link rendered below.
        if (preopenedWindow && !preopenedWindow.closed) {
          preopenedWindow.location.href = s.authorize_url;
        }
      })
      .catch((e) => { if (!cancelled) onError(e instanceof Error ? e.message : String(e)); });
    return () => { cancelled = true; };
  }, [onError, preopenedWindow]);

  // Poll status while we have a session id.
  useEffect(() => {
    if (!start) return undefined;
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
    if (start) void api.codexPkceCancel(start.session_id);
    onCancel();
  }, [start, onCancel]);

  const handlePaste = useCallback(async () => {
    if (!start) return;
    setSubmitting(true);
    try {
      await api.codexPkceManual(start.session_id, pasteCode.trim());
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e));
    } finally {
      setSubmitting(false);
    }
  }, [start, pasteCode, onError]);

  if (!start) {
    return <div className="codex-login-info">Starting…</div>;
  }

  return (
    <div className="codex-login-flow">
      <div className="codex-login-info">
        A browser tab should have opened. Sign in to continue.
        <div className="codex-login-link">
          <a href={start.authorize_url} target="_blank" rel="noopener noreferrer">
            Open sign-in page
          </a>
        </div>
        {!start.loopback_bound && (
          <div className="codex-login-warning">
            Couldn't bind localhost:{start.callback_port}; browser callback won't reach Phoenix.
            Use the manual paste below.
          </div>
        )}
      </div>

      <div className="codex-login-status">Waiting for sign-in…</div>

      <button
        type="button"
        className="login-button login-button-secondary codex-login-paste-toggle"
        onClick={() => setShowPaste((v) => !v)}
      >
        {showPaste ? 'Hide manual paste' : 'Browser callback didn’t fire? Paste code'}
      </button>

      {showPaste && (
        <div className="codex-login-paste">
          <p className="codex-login-help">
            After signing in, your browser is redirected to{' '}
            <code>{start.redirect_uri}?code=…&amp;state=…</code>. Copy the value of the{' '}
            <code>code</code> query parameter and paste it here.
          </p>
          <input
            type="text"
            className="login-input"
            placeholder="Paste auth code"
            value={pasteCode}
            onChange={(e) => setPasteCode(e.target.value)}
            disabled={submitting}
          />
          <button
            type="button"
            className="login-button"
            onClick={handlePaste}
            disabled={submitting || pasteCode.trim().length === 0}
          >
            {submitting ? 'Submitting…' : 'Submit code'}
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
  const startedRef = useRef(false);

  useEffect(() => {
    if (startedRef.current) return;
    startedRef.current = true;
    let cancelled = false;
    api.codexDeviceStart()
      .then((s) => { if (!cancelled) setStart(s); })
      .catch((e) => { if (!cancelled) onError(e instanceof Error ? e.message : String(e)); });
    return () => { cancelled = true; };
  }, [onError]);

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
    return <div className="codex-login-info">Requesting device code…</div>;
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
          Sign in with your ChatGPT account. Phoenix will detect completion automatically.
        </li>
      </ol>
      <div className="codex-login-status">
        Waiting (expires in {Math.floor(start.timeout_secs / 60)} minutes)…
      </div>
      <button type="button" className="codex-login-cancel" onClick={handleCancel}>
        Cancel
      </button>
    </div>
  );
}
