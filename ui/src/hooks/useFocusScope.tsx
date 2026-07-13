/* eslint-disable react-refresh/only-export-components -- context provider + hooks in one file is idiomatic React */
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';

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

type KeyboardRouterLayer = 'modal' | 'viewer' | 'passive-content';
type KeyboardRouterKey = 'mod+f' | 'Escape';

interface KeyboardShortcutRegistration {
  id: string;
  layer: KeyboardRouterLayer;
  key: KeyboardRouterKey;
  scopeId?: string | null;
  allowWhenNoActiveScope?: boolean;
  enabled?: boolean;
  dialogOpen?: boolean;
  handler: (event: KeyboardEvent) => void;
}

interface KeyboardRouterCommands {
  registerShortcut(registration: KeyboardShortcutRegistration): () => void;
}

const noopCommands: FocusScopeCommands = {
  pushScope: () => {},
  popScope: () => {},
};

const noopKeyboardRouter: KeyboardRouterCommands = {
  registerShortcut: () => () => {},
};

const defaultState: FocusScopeState = {
  isActiveScope: () => false,
  activeScope: null,
  hasActiveScope: false,
};

const FocusScopeCommandsContext = createContext<FocusScopeCommands>(noopCommands);
const FocusScopeStateContext = createContext<FocusScopeState>(defaultState);
const KeyboardRouterContext = createContext<KeyboardRouterCommands>(noopKeyboardRouter);

const LAYER_PRIORITY: Record<KeyboardRouterLayer, number> = {
  modal: 3,
  viewer: 2,
  'passive-content': 1,
};

function matchesShortcut(event: KeyboardEvent, key: KeyboardRouterKey): boolean {
  if (key === 'Escape') return event.key === 'Escape';
  return (event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'f';
}

function isViewerFindInputTarget(target: EventTarget | null): boolean {
  const element = target as HTMLElement | null;
  return Boolean(element instanceof HTMLElement && element.dataset['viewerFindInput'] === 'true');
}

function isEditableTarget(target: EventTarget | null): boolean {
  const element = target as HTMLElement | null;
  if (!element || !(element instanceof HTMLElement)) return false;
  if (isViewerFindInputTarget(element)) return false;
  const tag = element.tagName;
  return tag === 'INPUT' || tag === 'TEXTAREA' || element.isContentEditable;
}

export function FocusScopeProvider({ children }: { children: ReactNode }) {
  const [scopes, setScopes] = useState<string[]>([]);
  const registrationsRef = useRef(new Map<string, KeyboardShortcutRegistration>());

  const pushScope = useCallback((id: string) => {
    setScopes(prev => [...prev.filter(s => s !== id), id]);
  }, []);

  const popScope = useCallback((id: string) => {
    setScopes(prev => prev.filter(s => s !== id));
  }, []);

  const registerShortcut = useCallback((registration: KeyboardShortcutRegistration) => {
    registrationsRef.current.set(registration.id, registration);
    return () => {
      const current = registrationsRef.current.get(registration.id);
      if (current === registration) registrationsRef.current.delete(registration.id);
    };
  }, []);

  const scopesRef = useRef(scopes);
  scopesRef.current = scopes;

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const currentScopes = scopesRef.current;
      const activeScope = currentScopes.length > 0 ? currentScopes[currentScopes.length - 1]! : null;
      const matchingRegistrations = Array.from(registrationsRef.current.values())
        .filter(registration => registration.enabled !== false)
        .filter(registration => matchesShortcut(event, registration.key));
      if (matchingRegistrations.length === 0) return;

      const eligible = matchingRegistrations.filter(registration => {
        if (registration.dialogOpen) return false;
        const ownsScope = registration.scopeId == null
          ? true
          : activeScope === registration.scopeId || (registration.allowWhenNoActiveScope && activeScope === null);
        if (!ownsScope) return false;
        if (registration.key === 'mod+f' && isEditableTarget(event.target)) return false;
        return true;
      });
      if (eligible.length === 0) return;

      eligible.sort((a, b) => {
        const priorityDelta = LAYER_PRIORITY[b.layer] - LAYER_PRIORITY[a.layer];
        if (priorityDelta !== 0) return priorityDelta;
        return a.id.localeCompare(b.id);
      });

      eligible[0]?.handler(event);
    };

    window.addEventListener('keydown', onKeyDown, true);
    return () => window.removeEventListener('keydown', onKeyDown, true);
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
  const keyboardRouter = useMemo<KeyboardRouterCommands>(() => ({ registerShortcut }), [registerShortcut]);

  return (
    <KeyboardRouterContext.Provider value={keyboardRouter}>
      <FocusScopeCommandsContext.Provider value={commands}>
        <FocusScopeStateContext.Provider value={state}>
          {children}
        </FocusScopeStateContext.Provider>
      </FocusScopeCommandsContext.Provider>
    </KeyboardRouterContext.Provider>
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

export function useKeyboardRouterShortcut(registration: KeyboardShortcutRegistration) {
  const { registerShortcut } = useContext(KeyboardRouterContext);
  useEffect(() => registerShortcut(registration), [registerShortcut, registration]);
}

/** Hook that registers a focus scope on mount and unregisters on unmount */
export function useRegisterFocusScope(id: string | null) {
  const { pushScope, popScope } = useFocusScopeCommands();
  useEffect(() => {
    if (id === null) return undefined;
    pushScope(id);
    return () => popScope(id);
  }, [id, pushScope, popScope]);
}
