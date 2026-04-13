import { useEffect, useState } from 'react';
import type { ModelInfo, CredentialStatus, ModelsResponse } from '../api';
import { subscribeModels } from '../modelsPoller';

interface UseModelsReturn {
  models: ModelInfo[];
  credentialStatus: CredentialStatus | null;
}

/**
 * Subscribe to the shared models/credential poller. Multiple components can
 * call this simultaneously; only one polling loop runs for the entire app.
 *
 * Returns `{ models, credentialStatus }` — both sourced from the same
 * `/api/models` response so they stay consistent.
 */
export function useModels(): UseModelsReturn {
  const [state, setState] = useState<ModelsResponse | null>(null);

  useEffect(() => {
    return subscribeModels(setState);
  }, []);

  return {
    models: state?.models ?? [],
    // Preserve ConversationPage's prior semantics: 'not_configured' is treated
    // as "no credential status known yet", not a distinct state.
    credentialStatus:
      state && state.credential_status !== 'not_configured'
        ? state.credential_status
        : null,
  };
}
