/* eslint-disable react-refresh/only-export-components -- context provider + hooks in one file is idiomatic React */
import { createContext, useContext, useCallback, useMemo, useState, useEffect, type ReactNode } from 'react';

interface FocusScopeCommands {
  pushScope(id: string): void;
  popScope(id: string): void;
}

interface FocusScopeState {
  isActiveScope(id: string): boolean;
  activeScope: string | null;
  hasActiveScope: boolean;
}

type FocusScopeContextValue = FocusScopeCommands & FocusScopeState;

const noopCommands: FocusScopeCommands = {
  pushScope: () => {},
  popScope: () => {},
};

const defaultState: FocusScopeState = {
  isActiveScope: () => false,
  activeScope: null,
  hasActiveScope: false,
};

const FocusScopeCommandsContext = createContext<FocusScopeCommands>(noopCommands);
const FocusScopeStateContext = createContext<FocusScopeState>(defaultState);

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

  const activeScope: string | null = scopes.length > 0 ? scopes[scopes.length - 1]! : null;

  const state = useMemo<FocusScopeState>(() => ({
    isActiveScope,
    activeScope,
    hasActiveScope: scopes.length > 0,
  }), [isActiveScope, activeScope, scopes.length]);

  const commands = useMemo<FocusScopeCommands>(() => ({ pushScope, popScope }), [pushScope, popScope]);

  return (
    <FocusScopeCommandsContext.Provider value={commands}>
      <FocusScopeStateContext.Provider value={state}>
        {children}
      </FocusScopeStateContext.Provider>
    </FocusScopeCommandsContext.Provider>
  );
}

export function useFocusScope(): FocusScopeContextValue {
  return {
    ...useContext(FocusScopeCommandsContext),
    ...useContext(FocusScopeStateContext),
  };
}

export function useFocusScopeCommands(): FocusScopeCommands {
  return useContext(FocusScopeCommandsContext);
}

/** Hook that registers a focus scope on mount and unregisters on unmount */
export function useRegisterFocusScope(id: string) {
  const { pushScope, popScope } = useFocusScopeCommands();
  useEffect(() => {
    pushScope(id);
    return () => popScope(id);
  }, [id, pushScope, popScope]);
}
