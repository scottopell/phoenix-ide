import type { ModelsResponse } from '../api';

interface LlmStatusBannerProps {
  models: ModelsResponse | null;
}

/**
 * Shows an inline banner when LLM access is degraded at startup.
 *
 * Two cases:
 *   - No LLM configured at all (no gateway, no API keys): onboarding prompt
 *   - Gateway configured but unreachable: warning with hint to restart gateway
 */
export function LlmStatusBanner({ models }: LlmStatusBannerProps) {
  if (!models) return null;

  if (!models.llm_configured) {
    return (
      <div className="llm-status-banner llm-status-banner--unconfigured">
        <span className="llm-status-banner__icon">!</span>
        <span className="llm-status-banner__text">
          No LLM configured. Set <code>ANTHROPIC_API_KEY</code> or{' '}
          <code>LLM_GATEWAY</code> and restart Phoenix.
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

  return null;
}
