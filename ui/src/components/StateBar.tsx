import { useState, useRef, useEffect } from 'react';
import { Link } from 'react-router-dom';
import type { Conversation, ConversationState, ModelInfo } from '../api';
import type { ConnectionState } from '../hooks';
import { getStateDescription } from '../utils';

// Thresholds (match backend constants)
const WARNING_THRESHOLD = 0.80;
const CONTINUATION_THRESHOLD = 0.90;

interface StateBarProps {
  conversation: Conversation | null;
  convState: ConversationState;
  connectionState: ConnectionState;
  connectionAttempt: number;
  nextRetryIn: number | null;
  contextWindowUsed: number;
  /** Model's maximum context window in tokens */
  modelContextWindow: number;
  /** Available models from the API (used to detect 1M upgrade availability) */
  availableModels?: ModelInfo[];
  onRetryNow?: () => void;
  /** Callback to manually trigger continuation */
  onTriggerContinuation?: () => void;
  /** Callback to upgrade the conversation to a 1M model variant */
  onUpgradeModel?: (newModelId: string) => void;
}

/** Abbreviate model ID: "claude-sonnet-4-6" -> "sonnet-4.6", "gpt-4o" -> "gpt-4o"
 *  For 1M variants, strip the "-1m" suffix (the 1M badge handles display). */
function abbreviateModel(model: string): string {
  // Claude models: strip "claude-" prefix, strip "-1m" suffix, convert trailing version hyphen to dot
  if (!model.startsWith('claude-')) return model;
  let inner = model.slice(7); // strip "claude-"
  if (inner.endsWith('-1m')) {
    inner = inner.slice(0, -3);
  }
  const lastHyphen = inner.lastIndexOf('-');
  if (lastHyphen > 0 && /^\d+$/.test(inner.slice(lastHyphen + 1))) {
    return inner.slice(0, lastHyphen) + '.' + inner.slice(lastHyphen + 1);
  }
  return inner;
}

/** Extract project name from cwd, project_name field, or worktree path */
function getProjectName(conversation: Conversation): string | null {
  // Prefer explicit project_name from backend
  if (conversation.project_name) return conversation.project_name;

  // For non-work modes, extract from cwd
  const cwd = conversation.cwd;
  if (!cwd) return null;

  // Skip worktree UUIDs -- they're meaningless
  if (cwd.includes('.phoenix/worktrees/')) return null;

  const parts = cwd.replace(/\/$/, '').split('/');
  return parts[parts.length - 1] || null;
}

export function StateBar({
  conversation,
  convState,
  connectionState,
  connectionAttempt,
  nextRetryIn,
  contextWindowUsed,
  modelContextWindow,
  availableModels,
  onRetryNow,
  onTriggerContinuation,
  onUpgradeModel,
}: StateBarProps) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [upgradeConfirm, setUpgradeConfirm] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const upgradeRef = useRef<HTMLSpanElement>(null);

  // Close menu on outside click
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

  // Close upgrade confirmation on outside click
  useEffect(() => {
    if (!upgradeConfirm) return;
    const handleClick = (e: MouseEvent) => {
      if (upgradeRef.current && !upgradeRef.current.contains(e.target as Node)) {
        setUpgradeConfirm(false);
      }
    };
    document.addEventListener('mousedown', handleClick);
    return () => document.removeEventListener('mousedown', handleClick);
  }, [upgradeConfirm]);

  let dotClass = 'dot';
  let stateText = '';

  if (!conversation) {
    dotClass += ' hidden';
    stateText = '';
  } else {
    // Determine dot and text based on connection state first
    switch (connectionState) {
      case 'disconnected':
        dotClass += ' connecting';
        stateText = 'connecting...';
        break;

      case 'connecting':
        dotClass += ' connecting';
        stateText = 'connecting...';
        break;

      case 'reconnecting':
        dotClass += ' reconnecting';
        stateText = `reconnecting (${connectionAttempt})...`;
        break;

      case 'offline':
        dotClass += ' offline';
        stateText = 'offline';
        break;

      case 'reconnected':
        dotClass += ' reconnected';
        stateText = 'reconnected';
        break;

      case 'connected': {
        // When connected, show agent state
        switch (convState.type) {
          case 'idle':
            dotClass += ' idle';
            stateText = 'ready';
            break;
          case 'terminal':
            dotClass += ' terminal';
            stateText = 'completed';
            break;
          case 'awaiting_task_approval':
            dotClass += ' approval';
            stateText = 'awaiting approval';
            break;
          case 'awaiting_user_response':
            dotClass += ' approval';
            stateText = 'awaiting response';
            break;
          case 'error':
            dotClass += ' error';
            stateText = 'error';
            break;
          case 'context_exhausted':
            dotClass += ' error';
            stateText = 'context full';
            break;
          case 'awaiting_llm': case 'llm_requesting': case 'tool_executing':
          case 'awaiting_sub_agents': case 'awaiting_continuation':
          case 'cancelling': case 'cancelling_tool': case 'cancelling_sub_agents':
            dotClass += ' working';
            stateText = getStateDescription(convState);
            break;
          default: convState satisfies never;
        }
        break;
      }

      default:
        dotClass += ' connecting';
        stateText = 'connecting...';
    }
  }

  const showOfflineBanner = connectionState === 'offline' && nextRetryIn !== null;

  // Context window indicator - use model-specific limit
  const maxTokens = modelContextWindow || 200_000; // Fallback for legacy
  const contextPercent = Math.min((contextWindowUsed / maxTokens) * 100, 100);
  const contextWarning = contextPercent / 100 >= WARNING_THRESHOLD;
  const contextCritical = contextPercent / 100 >= CONTINUATION_THRESHOLD;

  let contextClass = 'context-indicator';
  if (contextCritical) {
    contextClass += ' critical';
  } else if (contextWarning) {
    contextClass += ' warning';
  }

  const formatTokens = (n: number): string => {
    if (n >= 1000) {
      return `${(n / 1000).toFixed(0)}k`;
    }
    return n.toString();
  };

  const tooltipText = `Context window usage: ${formatTokens(contextWindowUsed)} / ${formatTokens(maxTokens)} tokens (${contextPercent.toFixed(1)}%). When full, the conversation will need to be summarized.`;

  // Show menu trigger only when warning threshold reached and in idle state
  const canTriggerContinuation = contextWarning && convState.type === 'idle' && onTriggerContinuation;

  const handleTriggerContinuation = () => {
    setMenuOpen(false);
    onTriggerContinuation?.();
  };

  // Derived display values
  const mode = conversation?.conv_mode_label?.toLowerCase();
  const isWork = mode === 'work';
  const isExplore = mode === 'explore';
  const modeLabel = conversation?.conv_mode_label;
  const modeSuffix = isExplore ? ' (read-only)' : '';
  const modeClass = `statebar-mode statebar-mode--${mode}`;
  const is1m = conversation?.model?.endsWith('-1m') ?? false;
  const modelAbbrev = conversation ? abbreviateModel(conversation.model.replace(/-1m$/, '')) : '';
  const projectName = conversation ? getProjectName(conversation) : null;

  // Model upgrade detection: check if a 1M variant exists for the current model
  const currentModel = conversation?.model ?? '';
  const is1m = currentModel.endsWith('-1m');
  const upgradeModelId = is1m ? null : currentModel + '-1m';
  const canUpgrade = !!(
    upgradeModelId &&
    availableModels?.some(m => m.id === upgradeModelId) &&
    convState.type === 'idle' &&
    onUpgradeModel
  );

  const handleUpgradeClick = () => {
    if (!canUpgrade) return;
    setUpgradeConfirm(true);
  };

  const handleUpgradeConfirm = () => {
    if (!upgradeModelId || !onUpgradeModel) return;
    setUpgradeConfirm(false);
    onUpgradeModel(upgradeModelId);
  };

  // Git delta badges
  const ahead = conversation?.commits_ahead;
  const behind = conversation?.commits_behind;
  const baseBranch = conversation?.base_branch;
  const branchName = conversation?.branch_name;

  return (
    <>
      <header id="state-bar">
        <div id="state-bar-left">
          {conversation ? (
            <>
              {/* Line 1: nav slug + mode + model */}
              <div className="statebar-line1">
                <Link to="/" className="statebar-slug" title="Back to conversations">
                  <span className="back-arrow">&larr;</span>
                  <span className="slug-text">{conversation.slug}</span>
                </Link>
                {modeLabel && (
                  <span className={modeClass} title={
                    isExplore ? 'Read-only mode (git project)' :
                    isWork ? 'Write mode (task branch)' :
                    'Full access (no git workflow)'
                  }>
                    {modeLabel}{modeSuffix}
                  </span>
                )}
                <span className="conv-model" title={`Model: ${conversation.model}`}>
                  {modelAbbrev}
                  {is1m && <span className="model-1m-badge">1M</span>}
                </span>
                {canUpgrade && (
                  <span className="model-upgrade" ref={upgradeRef}>
                    {upgradeConfirm ? (
                      <span className="model-upgrade-confirm">
                        <span className="model-upgrade-prompt">Switch to 1M?</span>
                        <button
                          className="model-upgrade-yes"
                          onClick={handleUpgradeConfirm}
                          title="Upgrade to 1M context window"
                        >
                          Yes
                        </button>
                        <button
                          className="model-upgrade-no"
                          onClick={() => setUpgradeConfirm(false)}
                        >
                          No
                        </button>
                      </span>
                    ) : (
                      <button
                        className="model-upgrade-btn"
                        onClick={handleUpgradeClick}
                        title="Upgrade to 1M context window"
                      >
                        1M
                      </button>
                    )}
                  </span>
                )}
              </div>

              {/* Line 2: git info (Work/Explore) or project name */}
              {(branchName || projectName) && (
                <div className="statebar-line2">
                  {branchName && baseBranch && (
                    <span className="git-flow" title={`${baseBranch} <- ${branchName}`}>
                      <span className="git-base">{baseBranch}</span>
                      <span className="git-arrow">&larr;</span>
                      <span className="git-branch">{branchName}</span>
                    </span>
                  )}
                  {branchName && !baseBranch && (
                    <span className="git-branch-solo" title={`Branch: ${branchName}`}>
                      {branchName}
                    </span>
                  )}
                  {ahead != null && ahead > 0 && (
                    <span
                      className="git-badge git-badge--ahead"
                      title={`${ahead} commit(s) ahead of ${baseBranch || 'base'}`}
                    >
                      +{ahead}
                    </span>
                  )}
                  {behind != null && behind > 0 && (
                    <span
                      className="git-badge git-badge--behind"
                      title={`${behind} commit(s) behind ${baseBranch || 'base'} -- may need rebase`}
                    >
                      -{behind}
                    </span>
                  )}
                  {projectName && (
                    <span className="statebar-project" title={conversation.cwd}>
                      {projectName}
                    </span>
                  )}
                </div>
              )}
            </>
          ) : (
            <span className="statebar-slug">&mdash;</span>
          )}
        </div>
        <div id="state-bar-right">
          <div id="state-indicator">
            <span id="state-dot" className={dotClass}></span>
            <span id="state-text">{stateText}</span>
          </div>
          {conversation && contextWindowUsed > 0 && (
            <div
              className={contextClass}
              title={tooltipText}
              ref={menuRef}
            >
              <div
                className="context-bar-wrapper"
                onClick={() => canTriggerContinuation && setMenuOpen(!menuOpen)}
                style={{ cursor: canTriggerContinuation ? 'pointer' : 'default' }}
              >
                <div className="context-bar">
                  <div
                    className="context-fill"
                    style={{ width: `${contextPercent}%` }}
                  />
                </div>
                <span className="context-label">{formatTokens(contextWindowUsed)}</span>
                {canTriggerContinuation && (
                  <span className="context-menu-indicator">&#9660;</span>
                )}
              </div>
              {menuOpen && canTriggerContinuation && (
                <div className="context-menu">
                  <button
                    className="context-menu-item"
                    onClick={handleTriggerContinuation}
                  >
                    End & summarize conversation
                  </button>
                  <div className="context-menu-hint">
                    Creates a summary to continue in a new conversation
                  </div>
                </div>
              )}
            </div>
          )}
        </div>
      </header>
      {showOfflineBanner && (
        <div className="offline-banner">
          <span className="offline-banner-text">
            Connection lost. Reconnecting in {nextRetryIn}s...
          </span>
          {onRetryNow && (
            <button className="offline-banner-retry" onClick={onRetryNow}>
              Retry now
            </button>
          )}
        </div>
      )}
    </>
  );
}
