import { useCallback, useEffect, useState } from 'react';
import { api, type CodexLoginPreflight } from '../api';
import { subscribeModels } from '../modelsPoller';

export function useCodexPreflight(clearOnError = false) {
  const [codexPreflight, setCodexPreflight] = useState<CodexLoginPreflight | null>(null);

  const refetchCodexPreflight = useCallback(() => {
    api.codexLoginPreflight()
      .then((preflight) => setCodexPreflight(preflight))
      .catch(() => {
        if (clearOnError) setCodexPreflight(null);
      });
  }, [clearOnError]);

  useEffect(() => {
    refetchCodexPreflight();
    let lastConfigured: boolean | null = null;
    const unsubscribe = subscribeModels((models) => {
      if (lastConfigured !== null && lastConfigured !== models.llm_configured) {
        refetchCodexPreflight();
      }
      lastConfigured = models.llm_configured;
    });
    return () => { unsubscribe(); };
  }, [refetchCodexPreflight]);

  return { codexPreflight, refetchCodexPreflight };
}
