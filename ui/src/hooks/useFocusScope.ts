import { createContext, useContext, useCallback, useState, useEffect, type ReactNode } from 'react';

interface FocusScopeContextValue {
  pushScope(id: string): void;
  popScope(id: string): void;
  isActiveScope(id: string): boolean;
  activeScope: string | null;
  hasActiveScope: boolean;
}

const FocusScopeContext = createContext<FocusScopeContextValue>({
  pushScope: () => {},
  popScope: () => {},
  isActiveScope: () => false,
  activeScope: null,
  hasActiveScope: false,
});

export function FocusScopeProvider({ children }: { children: ReactNode }) {
  const [scopes, setScopes] = useState<string[]>([]);

  const pushScope = useCallback((id: string) => {
    setScopes(prev => [...prev.filter(s => s !== id), id]);
  }, []);

  const popScope = useCallback((id: string) => {
    setScopes(prev => prev.filter(s => s !== id));
  }, []);

  const isActiveScope = useCallback((id: string) => {
    return scopes.length > 0 && scopes[scopes.length - 1] === id;
  }, [scopes]);

  const activeScope = scopes.length > 0 ? scopes[scopes.length - 1] : null;

  return (
    <FocusScopeContext.Provider value={{
      pushScope, popScope, isActiveScope,
      activeScope,
      hasActiveScope: scopes.length > 0,
    }}>
      {children}
    </FocusScopeContext.Provider>
  );
}

export function useFocusScope() {
  return useContext(FocusScopeContext);
}

/** Hook that registers a focus scope on mount and unregisters on unmount */
export function useRegisterFocusScope(id: string) {
  const { pushScope, popScope } = useFocusScope();
  useEffect(() => {
    pushScope(id);
    return () => popScope(id);
  }, [id, pushScope, popScope]);
}
