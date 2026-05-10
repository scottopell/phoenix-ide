import { Suspense, lazy, useState } from 'react';
import type { ModelsResponse } from '../api';
import { useAutoAuth } from '../hooks';

const CredentialHelperPanel = lazy(() =>
  import('./CredentialHelperPanel').then(m => ({ default: m.CredentialHelperPanel }))
);

const CodexLoginPanel = lazy(() =>
  import('./CodexLoginPanel').then(m => ({ default: m.CodexLoginPanel }))
);

interface LlmStatusBannerProps {
  models: ModelsResponse | null;
}

export function LlmStatusBanner({ models }: LlmStatusBannerProps) {
  const credentialStatus = models?.credential_status ?? null;
  const { showAuthPanel, setShowAuthPanel } = useAutoAuth(credentialStatus);
  const [showCodexPanel, setShowCodexPanel] = useState(false);

  if (!models) return null;

  if (!models.llm_configured) {
    if (showCodexPanel) {
      return (
        <Suspense fallback={<div className="llm-status-banner llm-status-banner--unconfigured">Loading…</div>}>
          <div className="llm-status-banner llm-status-banner--unconfigured llm-status-banner--expanded">
            <div className="llm-status-banner__panel-header">
              <strong>Sign in with Codex</strong>
            </div>
            <CodexLoginPanel onDismiss={() => setShowCodexPanel(false)} />
          </div>
        </Suspense>
      );
    }
    return (
      <div className="llm-status-banner llm-status-banner--unconfigured">
        <span className="llm-status-banner__icon">!</span>
        <span className="llm-status-banner__text">
          No LLM configured.{' '}
          <button
            type="button"
            className="llm-status-banner__action"
            onClick={() => setShowCodexPanel(true)}
          >
            Sign in with Codex
          </button>
          {', '}or set <code>ANTHROPIC_API_KEY</code> / <code>LLM_GATEWAY</code> and restart.
        </span>
      </div>
    );
  }

  if (models.gateway_status === 'unreachable') {
    return (
      <div className="llm-status-banner llm-status-banner--warning">
        <span className="llm-status-banner__icon">!</span>
        <span className="llm-status-banner__text">
          LLM gateway unreachable. Start your gateway and refresh.
        </span>
      </div>
    );
  }

  if (showAuthPanel && credentialStatus && credentialStatus !== 'not_configured' && credentialStatus !== 'valid') {
    return (
      <Suspense fallback={null}>
        <CredentialHelperPanel
          active={showAuthPanel}
          onDismiss={() => {
            setShowAuthPanel(false);
          }}
        />
      </Suspense>
    );
  }

  return null;
}
