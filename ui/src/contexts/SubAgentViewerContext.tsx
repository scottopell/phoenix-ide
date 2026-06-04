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
import { useLocation } from 'react-router-dom';

/**
 * Identity + display context for the sub-agent currently shown in the
 * side-docked viewer. `agentId` is the sub-agent's conversation_id (the
 * `spawn_agents` invariant — see runtime/executor.rs), which the read-only
 * inline stream consumes directly; the rest is what the parent already knows
 * about the spawn so the panel header reads the same as the card.
 */
export interface OpenedSubAgent {
  agentId: string;
  task: string;
  running: boolean;
  /** Final outcome text, when the sub-agent has completed. */
  resultText: string;
}

interface SubAgentViewerValue {
  opened: OpenedSubAgent | null;
  open: (agent: OpenedSubAgent) => void;
  close: () => void;
}

const SubAgentViewerContext = createContext<SubAgentViewerValue | null>(null);

/**
 * Holds which sub-agent is open in the side panel. Scoped to the parent
 * conversation being viewed: navigating to a different route closes the
 * panel, since the opened sub-agent belongs to the parent you just left.
 */
export function SubAgentViewerProvider({ children }: { children: ReactNode }) {
  const [opened, setOpened] = useState<OpenedSubAgent | null>(null);
  const open = useCallback((agent: OpenedSubAgent) => setOpened(agent), []);
  const close = useCallback(() => setOpened(null), []);

  const location = useLocation();
  const pathRef = useRef(location.pathname);
  useEffect(() => {
    if (pathRef.current !== location.pathname) {
      pathRef.current = location.pathname;
      setOpened(null);
    }
  }, [location.pathname]);

  const value = useMemo(() => ({ opened, open, close }), [opened, open, close]);
  return (
    <SubAgentViewerContext.Provider value={value}>
      {children}
    </SubAgentViewerContext.Provider>
  );
}

/**
 * Null when no provider is mounted (e.g. the mobile layout, which has no
 * room to dock a side panel). Callers fall back to full-page navigation in
 * that case.
 */
// eslint-disable-next-line react-refresh/only-export-components
export function useSubAgentViewer(): SubAgentViewerValue | null {
  return useContext(SubAgentViewerContext);
}
