import { createContext, useContext } from 'react';
import type { ApplicationCapabilities } from '../generated/ApplicationCapabilities';

export const ApplicationCapabilitiesContext = createContext<ApplicationCapabilities | null>(null);

export function useApplicationCapabilities(): ApplicationCapabilities {
  const capabilities = useContext(ApplicationCapabilitiesContext);
  if (capabilities === null) throw new Error('Application capabilities require ApplicationBootstrap or an explicit fixture provider');
  return capabilities;
}
