import { useCallback, useMemo, type ReactNode } from 'react';
import { useLocalStorage } from '../hooks/useLocalStorage';
import {
  DensityContext,
  DENSITY_STORAGE_KEY,
  type Density,
} from '../hooks/useDensity';

export function DensityProvider({ children }: { children: ReactNode }) {
  const [density, setDensityValue] = useLocalStorage<Density>(DENSITY_STORAGE_KEY, 'full');

  const setDensity = useCallback(
    (next: Density) => setDensityValue(next),
    [setDensityValue],
  );

  const value = useMemo(() => ({ density, setDensity }), [density, setDensity]);

  return <DensityContext.Provider value={value}>{children}</DensityContext.Provider>;
}
