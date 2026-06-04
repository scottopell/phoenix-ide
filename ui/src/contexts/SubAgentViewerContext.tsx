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
 * Stable identity of the sub-agent currently shown in the side-docked viewer.
 * `agentId` is the sub-agent's conversation_id (the `spawn_agents` invariant —
 * see runtime/executor.rs), which the read-only stream consumes directly;
 * `task` is the header title. Live status (running / outcome) is intentionally
 * NOT stored here — the panel derives it from the sub-agent's own stream, so it
 * stays correct even after the parent's (virtualized) card unmounts.
 */
export interface OpenedSubAgent {
  agentId: string;
  task: string;
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
