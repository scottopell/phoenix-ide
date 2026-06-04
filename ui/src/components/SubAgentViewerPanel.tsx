import { useCallback, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { ChildConversationActivity, navigateToSubAgent } from './MessageComponents';
import type { OpenedSubAgent } from '../contexts/SubAgentViewerContext';

const CloseIcon = () => (
  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <line x1="18" y1="6" x2="6" y2="18" />
    <line x1="6" y1="6" x2="18" y2="18" />
  </svg>
);
const ExternalLinkIcon = () => (
  <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
    <polyline points="15 3 21 3 21 9" />
    <line x1="10" y1="14" x2="21" y2="3" />
  </svg>
);

interface Props {
  opened: OpenedSubAgent;
  onClose: () => void;
  /** Width in px — driven by useResizablePane in the dock. */
  width?: number | undefined;
}

/**
 * Side-docked, read-only viewer for a sub-agent's full conversation. Renders
 * alongside the parent conversation so opening a sub-agent doesn't drop the
 * user onto a context-less page. The body reuses the same read-only transcript
 * renderer as the inline peek (`ChildConversationActivity`), un-truncated.
 *
 * There is intentionally no composer: a completed sub-agent takes no further
 * input, and a running one is driven by its parent — the panel is for reading.
 */
export function SubAgentViewerPanel({ opened, onClose, width }: Props) {
  const { agentId, task, running, resultText } = opened;
  const navigate = useNavigate();
  const [opening, setOpening] = useState(false);
  const inFlight = useRef(false);

  const openFullPage = useCallback(async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    setOpening(true);
    try {
      await navigateToSubAgent(agentId, navigate);
    } catch {
      // Transient failure — leave the panel open so the user can retry.
    } finally {
      inFlight.current = false;
      setOpening(false);
    }
  }, [agentId, navigate]);

  return (
    <aside
      className="subagent-viewer-panel"
      style={width !== undefined ? { width: `${width}px`, minWidth: `${width}px` } : undefined}
    >
      <div className="subagent-viewer-header">
        <div className="subagent-viewer-heading">
          <span className="subagent-viewer-title" title={task}>{task}</span>
          <span className="subagent-viewer-subtitle">
            sub-agent · read-only{running ? ' · live' : ''}
          </span>
        </div>
        <button
          type="button"
          className="subagent-viewer-action"
          onClick={openFullPage}
          disabled={opening}
          title="Open as a full page"
          aria-label="Open sub-agent as a full page"
        >
          <ExternalLinkIcon />
        </button>
        <button
          type="button"
          className="subagent-viewer-action"
          onClick={onClose}
          title="Close sub-agent viewer"
          aria-label="Close sub-agent viewer"
        >
          <CloseIcon />
        </button>
      </div>
      <div className="subagent-viewer-content">
        <ChildConversationActivity agentId={agentId} expanded running={running} full />
        {resultText && (
          <div className="subagent-final-result">
            <div className="subagent-final-result-label">final outcome</div>
            <div className="subagent-viewer-result-text">{resultText}</div>
          </div>
        )}
      </div>
    </aside>
  );
}
