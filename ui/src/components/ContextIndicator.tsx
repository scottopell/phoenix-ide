import { useState, useRef, useEffect } from 'react';
import { api } from '../api';
import type { ConversationUsage, UsageTotals } from '../api';

// Thresholds (match backend constants)
const WARNING_THRESHOLD = 0.80;
const CONTINUATION_THRESHOLD = 0.90;

interface ContextIndicatorProps {
  used: number;
  max: number;
  conversationId: string;
  /** When provided AND the warning threshold is crossed, a trigger menu appears with an
   *  "End & summarize" action that invokes this callback. */
  onTriggerContinuation?: (() => void) | undefined;
}

const formatTokens = (n: number): string => {
  if (n >= 1000) {
    return `${(n / 1000).toFixed(0)}k`;
  }
  return n.toString();
};

function UsageStatsSection({ label, totals }: { label: string; totals: UsageTotals }) {
  return (
    <div className="usage-stats-section">
      <div className="usage-stats-label">{label}</div>
      <table className="usage-stats-table">
        <tbody>
          <tr>
            <td className="usage-stat-name">Input</td>
            <td className="usage-stat-value">{totals.input_tokens.toLocaleString()}</td>
          </tr>
          <tr>
            <td className="usage-stat-name">From cache</td>
            <td className="usage-stat-value">{totals.cache_read_tokens.toLocaleString()}</td>
          </tr>
          <tr>
            <td className="usage-stat-name">Cache writes</td>
            <td className="usage-stat-value">{totals.cache_creation_tokens.toLocaleString()}</td>
          </tr>
          <tr>
            <td className="usage-stat-name">Output</td>
            <td className="usage-stat-value">{totals.output_tokens.toLocaleString()}</td>
          </tr>
        </tbody>
      </table>
      <div className="usage-stats-turns">{totals.turns} {totals.turns === 1 ? 'turn' : 'turns'}</div>
    </div>
  );
}

export function ContextIndicator({ used, max, conversationId, onTriggerContinuation }: ContextIndicatorProps) {
  const [panelOpen, setPanelOpen] = useState(false);
  const [usage, setUsage] = useState<ConversationUsage | null>(null);
  const [usageLoading, setUsageLoading] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!panelOpen) return;
    const handleClick = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setPanelOpen(false);
      }
    };
    document.addEventListener('mousedown', handleClick);
    return () => document.removeEventListener('mousedown', handleClick);
  }, [panelOpen]);

  useEffect(() => {
    if (!panelOpen) return;
    setUsageLoading(true);
    api.getConversationUsage(conversationId)
      .then(data => { setUsage(data); setUsageLoading(false); })
      .catch(() => setUsageLoading(false));
  }, [panelOpen, conversationId]);

  const percent = Math.min((used / max) * 100, 100);
  const fraction = percent / 100;
  const warning = fraction >= WARNING_THRESHOLD;
  const critical = fraction >= CONTINUATION_THRESHOLD;
  const canTrigger = warning && !!onTriggerContinuation;

  let className = 'context-indicator';
  if (critical) className += ' critical';
  else if (warning) className += ' warning';

  const tooltipText = `Context window usage: ${formatTokens(used)} / ${formatTokens(max)} tokens (${percent.toFixed(1)}%). When full, the conversation will need to be summarized.`;

  const handleTrigger = () => {
    setPanelOpen(false);
    onTriggerContinuation?.();
  };

  return (
    <div className={className} title={tooltipText} ref={menuRef}>
      <div
        className="context-bar-wrapper"
        onClick={() => setPanelOpen(!panelOpen)}
      >
        <div className="context-bar">
          <div className="context-fill" style={{ width: `${percent}%` }} />
        </div>
        <span className="context-label">{formatTokens(used)}</span>
        <span className="context-menu-indicator">&#9660;</span>
      </div>
      {panelOpen && (
        <div className="context-menu usage-panel">
          <div className="usage-panel-stats">
            {usageLoading ? (
              <div className="usage-panel-loading">Loading...</div>
            ) : usage ? (
              <>
                <UsageStatsSection label="This conversation" totals={usage.own} />
                {usage.total.turns > usage.own.turns && (
                  <UsageStatsSection label="Total incl. sub-agents" totals={usage.total} />
                )}
              </>
            ) : (
              <div className="usage-panel-empty">No usage data yet</div>
            )}
          </div>
          {canTrigger && (
            <>
              <div className="context-menu-divider" />
              <button className="context-menu-item" onClick={handleTrigger}>
                End &amp; summarize conversation
              </button>
              <div className="context-menu-hint">
                Creates a summary to continue in a new conversation
              </div>
            </>
          )}
        </div>
      )}
    </div>
  );
}
