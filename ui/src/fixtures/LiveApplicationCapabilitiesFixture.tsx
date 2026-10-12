import type { ReactNode } from 'react';
import { ApplicationCapabilitiesContext } from '../contexts/ApplicationCapabilitiesContext';

export function LiveApplicationCapabilitiesFixture({ children }: { children: ReactNode }) {
  return <ApplicationCapabilitiesContext.Provider value={{
    persisted_member_read: 'available', live_stream: 'available', conversation_activation: 'available',
    user_input: 'available', telemetry: 'available', provider_catalog: 'available', host_resource_observations: 'available',
  }}>{children}</ApplicationCapabilitiesContext.Provider>;
}
