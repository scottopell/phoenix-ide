import { useEffect, useState, type ReactNode } from 'react';
import { api } from '../api';
import type { ApplicationCapabilities } from '../generated/ApplicationCapabilities';
import { ApplicationCapabilitiesContext } from './ApplicationCapabilitiesContext';

export function ApplicationBootstrap({ children }: { children: ReactNode }) {
  const [result, setResult] = useState<
    { status: 'loading' } | { status: 'ready'; capabilities: ApplicationCapabilities } | { status: 'failed'; error: string }
  >({ status: 'loading' });
  useEffect(() => {
    let cancelled = false;
    api.getCapabilities().then((capabilities) => {
      if (!cancelled) setResult({ status: 'ready', capabilities });
    }).catch((error: unknown) => {
      if (!cancelled) setResult({ status: 'failed', error: error instanceof Error ? error.message : String(error) });
    });
    return () => { cancelled = true; };
  }, []);
  if (result.status === 'loading') return <main role="status">Loading application capabilities…</main>;
  if (result.status === 'failed') return <main role="alert">Unable to load application capabilities: {result.error}</main>;
  return <ApplicationCapabilitiesContext.Provider value={result.capabilities}>{children}</ApplicationCapabilitiesContext.Provider>;
}