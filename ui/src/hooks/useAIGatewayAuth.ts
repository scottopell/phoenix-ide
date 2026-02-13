import { useState, useEffect, useCallback } from 'react';
import { api } from '../api';

export interface AuthState {
  status: 'not_required' | 'authenticated' | 'required' | 'in_progress' | 'failed';
  oauth_url?: string;
  device_code?: string;
  error?: string;
}

export function useAIGatewayAuth() {
  const [authState, setAuthState] = useState<AuthState | null>(null);

  // Check auth status on mount (only if AI Gateway enabled)
  // Backend returns 'not_required' for other LLM providers
  useEffect(() => {
    api
      .checkAIGatewayAuthStatus()
      .then(setAuthState)
      .catch((err) => console.error('Failed to check auth:', err));
  }, []);

  // Poll when auth in progress
  useEffect(() => {
    if (authState?.status === 'in_progress') {
      const interval = setInterval(() => {
        api
          .pollAIGatewayAuth()
          .then((state) => {
            setAuthState(state);
            if (state.status === 'authenticated') {
              // Could show success toast here
              console.log('AI Gateway authentication successful');
            }
          })
          .catch((err) => console.error('Poll failed:', err));
      }, 3000); // Poll every 3 seconds

      return () => clearInterval(interval);
    }
    return undefined;
  }, [authState?.status]);

  const initiateAuth = useCallback(async () => {
    try {
      const state = await api.initiateAIGatewayAuth();
      setAuthState(state);
    } catch (err) {
      console.error('Failed to initiate auth:', err);
    }
  }, []);

  return { authState, initiateAuth };
}
