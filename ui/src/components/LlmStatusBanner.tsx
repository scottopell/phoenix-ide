import { Suspense, lazy } from 'react';
import type { ModelsResponse } from '../api';
import { useAutoAuth } from '../hooks';

const CredentialHelperPanel = lazy(() =>
  import('./CredentialHelperPanel').then(m => ({ default: m.CredentialHelperPanel }))
);

interface LlmStatusBannerProps {
  models: ModelsResponse | null;
}

/**
 * Shows an inline banner when LLM access is degraded at startup.
 *
 * Three cases:
 *   - No LLM configured at all (no gateway, no API keys): onboarding prompt
 *   - Gateway configured but unreachable: warning with hint to restart gateway
 *   - Credential helper needs auth: shows the auth panel inline
 */
export function LlmStatusBanner({ models }: LlmStatusBannerProps) {
  const credentialStatus = models?.credential_status ?? null;
  const { showAuthPanel, setShowAuthPanel } = useAutoAuth(credentialStatus);

  if (!models) return null;

  if (!models.llm_configured) {
    return (
      <div className="llm-status-banner llm-status-banner--unconfigured">
        <span className="llm-status-banner__icon">!</span>
        <span className="llm-status-banner__text">
          No LLM configured. Run <code>claude login</code>, or set{' '}
          <code>ANTHROPIC_API_KEY</code> or <code>LLM_GATEWAY</code>, then restart Phoenix.
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
