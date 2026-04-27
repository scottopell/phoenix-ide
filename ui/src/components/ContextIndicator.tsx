import { useState, useRef, useEffect } from 'react';

// Thresholds (match backend constants)
const WARNING_THRESHOLD = 0.80;
const CONTINUATION_THRESHOLD = 0.90;

interface ContextIndicatorProps {
  used: number;
  max: number;
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

export function ContextIndicator({ used, max, onTriggerContinuation }: ContextIndicatorProps) {
  const [menuOpen, setMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!menuOpen) return;
    const handleClick = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setMenuOpen(false);
      }
    };
    document.addEventListener('mousedown', handleClick);
    return () => document.removeEventListener('mousedown', handleClick);
  }, [menuOpen]);

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
    setMenuOpen(false);
    onTriggerContinuation?.();
  };

  return (
    <div className={className} title={tooltipText} ref={menuRef}>
      <div
        className="context-bar-wrapper"
        onClick={() => canTrigger && setMenuOpen(!menuOpen)}
        style={{ cursor: canTrigger ? 'pointer' : 'default' }}
      >
        <div className="context-bar">
          <div className="context-fill" style={{ width: `${percent}%` }} />
        </div>
        <span className="context-label">{formatTokens(used)}</span>
        {canTrigger && (
          <span className="context-menu-indicator">&#9660;</span>
        )}
      </div>
      {menuOpen && canTrigger && (
        <div className="context-menu">
          <button className="context-menu-item" onClick={handleTrigger}>
            End &amp; summarize conversation
          </button>
          <div className="context-menu-hint">
            Creates a summary to continue in a new conversation
          </div>
        </div>
      )}
    </div>
  );
}
