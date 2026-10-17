/* eslint-disable react-refresh/only-export-components -- Capability context and startup boundary are colocated. */
import { createContext, useContext, useEffect, useState, type ReactNode } from 'react';
import * as v from 'valibot';
import type { ApplicationCapabilities } from '../generated/ApplicationCapabilities';
import type { CapabilityAvailability } from '../generated/CapabilityAvailability';

const availability = v.picklist(['available', 'unavailable']) satisfies v.GenericSchema<unknown, CapabilityAvailability>;
export const applicationCapabilitiesSchema = v.object({
  persisted_member_read: availability,
  live_stream: availability,
  conversation_activation: availability,
  user_input: availability,
  telemetry: availability,
  provider_catalog: availability,
  host_resource_observations: availability,
}) satisfies v.GenericSchema<unknown, ApplicationCapabilities>;
export const liveCapabilities: ApplicationCapabilities = {
  persisted_member_read: 'available', live_stream: 'available', conversation_activation: 'available',
  user_input: 'available', telemetry: 'available', provider_catalog: 'available', host_resource_observations: 'available',
};
export const ApplicationCapabilitiesContext = createContext(liveCapabilities);
export const useApplicationCapabilities = () => useContext(ApplicationCapabilitiesContext);
export function hasLiveApplication(capabilities: ApplicationCapabilities): boolean {
  return Object.values(capabilities).every(value => value === 'available');
}

export function ApplicationCapabilitiesProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<ApplicationCapabilities | 'loading' | 'error'>('loading');
  useEffect(() => {
    const controller = new AbortController();
    fetch('/api/capabilities', { signal: controller.signal, cache: 'no-store' })
      .then(async response => {
        if (!response.ok) throw new Error('Capability discovery failed');
        return v.parse(applicationCapabilitiesSchema, await response.json());
      })
      .then(value => { if (!controller.signal.aborted) setState(value); })
      .catch(() => { if (!controller.signal.aborted) setState('error'); });
    return () => controller.abort();
  }, []);
  if (state === 'loading') return <div role="status">Loading capabilities…</div>;
  if (state === 'error') return <div role="alert">Application capabilities unavailable. Reload to retry.</div>;
  return <ApplicationCapabilitiesContext.Provider value={state}>{children}</ApplicationCapabilitiesContext.Provider>;
}
