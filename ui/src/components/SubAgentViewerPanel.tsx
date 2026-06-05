import { useCallback, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { SubAgentTranscript, navigateToSubAgent } from './MessageComponents';
import { useConversationInlineStream } from '../hooks/useConversationInlineStream';
import { isAgentWorking } from '../utils';
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
 * user onto a context-less page.
 *
 * The panel owns the sub-agent's read-only stream itself (rather than reading a
 * snapshot of the parent's spawn card) and derives live status from that
 * stream's phase. This keeps the status correct even when the parent card
 * scrolls out of the virtualized message list and unmounts. The stream is
 * opened live (not phase-gated) so a just-spawned sub-agent still momentarily
 * `Idle` is followed once it starts working, and it self-closes when the
 * sub-agent finishes.
 *
 * There is intentionally no composer: a completed sub-agent takes no further
 * input, and a running one is driven by its parent — the panel is for reading.
 *
 * The dock keys this component by `agentId`, so switching sub-agents remounts
 * it with a fresh stream rather than showing the prior agent's atom under the
 * new title until the new snapshot lands.
 */
export function SubAgentViewerPanel({ opened, onClose, width }: Props) {
  const { agentId, task } = opened;
  const navigate = useNavigate();
  const [opening, setOpening] = useState(false);
  const inFlight = useRef(false);

  const inline = useConversationInlineStream(agentId, true, true);
  const running = isAgentWorking(inline.atom.phase);

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
        <SubAgentTranscript inline={inline} running={running} full />
      </div>
    </aside>
  );
}
