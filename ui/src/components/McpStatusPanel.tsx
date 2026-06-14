import { useState, useEffect, useCallback, useRef } from 'react';
import { api } from '../api';
import type { McpServerStatus } from '../api';
import './McpStatusPanel.css';

interface McpStatusPanelProps {
  /** Success / info path. Renders with the success / info styling
   *  (whatever the parent wires into this prop). */
  showToast: (message: string, duration?: number) => void;
  /** Failure path — renders with red `error` styling so the user
   *  can tell at a glance that something went wrong, vs the green
   *  status path. REQ-NOTIF-002 (specs/notifications/). */
  showError: (message: string, duration?: number) => void;
}

export function McpStatusPanel({ showToast, showError }: McpStatusPanelProps) {
  const [servers, setServers] = useState<McpServerStatus[]>([]);
  const [expanded, setExpanded] = useState(false);
  const [expandedServers, setExpandedServers] = useState<Set<string>>(new Set());
  const [reloading, setReloading] = useState(false);
  const [togglingServers, setTogglingServers] = useState<Set<string>>(new Set());
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);
  // Reload-retry polling: the backend reconnects retried servers in the
  // background (up to the connect timeout), so after a reload we keep polling
  // until every awaited server is `ready` or this deadline passes — long enough
  // for a slow reconnect to flip failed/pending → ready without a manual
  // refresh. `awaitingRef` is null while the reload request itself is in flight
  // (the awaited set isn't known yet), then the names being (re)connected.
  const reloadUntilRef = useRef<number>(0);
  const awaitingRef = useRef<Set<string> | null>(null);

  const fetchStatus = useCallback(async () => {
    try {
      const status = await api.getMcpStatus();
      setServers(status);
      return status.length;
    } catch {
      return 0;
    }
  }, []);

  // Whether a reload's retried servers have settled (all `ready`) or its window
  // has elapsed, so polling may stop. Outside a reload (deadline in the past)
  // this is vacuously true, preserving the connect-once polling behavior.
  const reloadSettled = useCallback((s: McpServerStatus[]) => {
    if (Date.now() > reloadUntilRef.current) return true;
    const awaiting = awaitingRef.current;
    if (awaiting === null) return false; // reload request still in flight
    const ready = new Set(s.filter(srv => srv.state === 'ready').map(srv => srv.name));
    return [...awaiting].every(name => ready.has(name));
  }, []);

  // Poll every 3s until servers are connected. Keep polling while any server
  // has a pending OAuth URL so the UI can update when auth completes.
  useEffect(() => {
    let cancelled = false;
    const shouldStopPolling = (s: McpServerStatus[]) =>
      s.length > 0 && s.every(srv => !srv.pending_oauth_url) && reloadSettled(s);

    fetchStatus().then(count => {
      if (cancelled) return;
      if (count > 0 && shouldStopPolling(servers)) return;
      pollRef.current = setInterval(async () => {
        await fetchStatus();
        // Re-evaluate stop condition after each fetch via the state update.
      }, 3000);
    });
    return () => {
      cancelled = true;
      if (pollRef.current) clearInterval(pollRef.current);
    };
  }, [fetchStatus]); // eslint-disable-line react-hooks/exhaustive-deps

  // Stop polling once all OAuth flows have resolved and any reload retry has
  // settled.
  useEffect(() => {
    if (
      servers.length > 0 &&
      servers.every(s => !s.pending_oauth_url) &&
      reloadSettled(servers) &&
      pollRef.current
    ) {
      clearInterval(pollRef.current);
      pollRef.current = null;
    }
  }, [servers, reloadSettled]);

  const handleReload = useCallback(async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (reloading) return;
    setReloading(true);
    // Open the retry-polling window (bounded by the backend connect timeout)
    // and mark the awaited set unknown until the reload request returns it.
    reloadUntilRef.current = Date.now() + 300_000;
    awaitingRef.current = null;
    // Synchronously drop stale pending-OAuth and failed entries from local
    // state: the reload retries them as background connects, so their new
    // outcome should repopulate from polling rather than the panel showing a
    // stale state. Connected servers are kept untouched.
    setServers(prev => prev.filter(s => s.state === 'ready'));
    // Ensure polling is active — connection happens as a background task on the
    // server, so the new OAuth URL won't be in the status we fetch immediately.
    if (!pollRef.current) {
      pollRef.current = setInterval(() => { void fetchStatus(); }, 3000);
    }
    try {
      const result = await api.reloadMcp();
      // The (re)connecting servers to await before polling settles: a slow one
      // flips to `ready` later, within the window above. `failed` is included
      // because a timed-out restart still has a background connect running that
      // may publish successfully and clear the failure.
      awaitingRef.current = new Set([
        ...result.added,
        ...result.restarted,
        ...result.failed.map(f => f.server),
      ]);
      await fetchStatus();
      const parts: string[] = [];
      if (result.added.length > 0) parts.push(`+${result.added.length} added`);
      if (result.removed.length > 0) parts.push(`-${result.removed.length} removed`);
      if (result.restarted.length > 0) parts.push(`↻${result.restarted.length} restarted`);
      if (result.failed.length > 0) parts.push(`!${result.failed.length} failed`);
      if (result.unchanged.length > 0) parts.push(`${result.unchanged.length} unchanged`);
      const message = `MCP reload: ${parts.join(', ') || 'no servers'}`;
      if (result.failed.length > 0) {
        const [firstFailure] = result.failed;
        const suffix = firstFailure
          ? ` (${firstFailure.server} ${firstFailure.action} failed)`
          : '';
        showError(`${message}${suffix}`, 5000);
      } else {
        showToast(message, 3000);
      }
      // Keep reloading=true until the next poll shows fresh OAuth content
      // (effect below) or the safety timeout fires.
    } catch {
      showError('MCP reload failed', 3000);
      setReloading(false);
      // The reload never landed; close the retry window so polling settles
      // normally instead of running for the full timeout.
      reloadUntilRef.current = 0;
      awaitingRef.current = null;
    }
  }, [reloading, fetchStatus, showToast, showError]);

  // Clear `reloading` once new content arrives, with a 5s safety timeout to
  // avoid a stuck spinner if the backend connection never emits anything.
  useEffect(() => {
    if (!reloading) return;
    if (servers.length > 0) {
      setReloading(false);
      return;
    }
    const t = setTimeout(() => setReloading(false), 5000);
    return () => clearTimeout(t);
  }, [reloading, servers.length]);

  const handleToggleEnabled = useCallback(async (serverName: string, currentlyEnabled: boolean) => {
    setTogglingServers(prev => new Set(prev).add(serverName));
    try {
      if (currentlyEnabled) {
        await api.disableMcpServer(serverName);
      } else {
        await api.enableMcpServer(serverName);
      }
      await fetchStatus();
      showToast(`${serverName}: ${currentlyEnabled ? 'disabled' : 'enabled'}`, 2000);
    } catch {
      showError(`Failed to ${currentlyEnabled ? 'disable' : 'enable'} ${serverName}`, 3000);
    } finally {
      setTogglingServers(prev => {
        const next = new Set(prev);
        next.delete(serverName);
        return next;
      });
    }
  }, [fetchStatus, showToast, showError]);

  const toggleServer = useCallback((name: string) => {
    setExpandedServers(prev => {
      const next = new Set(prev);
      if (next.has(name)) {
        next.delete(name);
      } else {
        next.add(name);
      }
      return next;
    });
  }, []);

  const readyServers = servers.filter(s => s.state === 'ready');
  const enabledReady = readyServers.filter(s => s.enabled);
  const totalTools = enabledReady.reduce((sum, s) => sum + s.tool_count, 0);
  const disabledCount = readyServers.filter(s => !s.enabled).length;

  const pendingOAuth = servers.filter(s => s.state === 'unauthorized');
  const failedServers = servers.filter(s => s.state === 'failed');

  if (servers.length === 0 && !expanded && !reloading) {
    return null;
  }

  return (
    <div className={`mcp-panel${expanded ? ' is-expanded' : ''}`}>
      {!reloading && pendingOAuth.map(s => (
        <div key={s.name} className="mcp-oauth-banner">
          <span className="mcp-oauth-label">Auth required:</span>
          <a
            href={s.pending_oauth_url}
            target="_blank"
            rel="noreferrer"
            className="mcp-oauth-link"
          >
            {s.name} &rarr; sign in
          </a>
          {s.auth_redirect_warning && (
            <span className="mcp-oauth-warning" title={s.auth_redirect_warning}>
              &#9888; {s.auth_redirect_warning}
            </span>
          )}
        </div>
      ))}
      {!reloading && failedServers.map(s => (
        <div key={s.name} className="mcp-error-banner">
          <span className="mcp-error-label">Failed:</span>
          <span className="mcp-error-text" title={s.last_error}>
            {s.name} &mdash; {s.last_error ?? 'connection failed'}
          </span>
        </div>
      ))}
      <button className="mcp-panel-header" onClick={() => setExpanded(!expanded)}>
        <span className={`mcp-panel-chevron ${expanded ? 'expanded' : ''}`}>&#9654;</span>
        <span className="mcp-panel-summary">
          {servers.length === 0
            ? 'No MCP servers'
            : enabledReady.length === 0 && pendingOAuth.length > 0
              ? <span className="mcp-auth-needed">MCP &middot; auth needed</span>
              : enabledReady.length === 0 && failedServers.length > 0
                ? <span className="mcp-failed-count">MCP &middot; {failedServers.length} failed</span>
                : <>
                    MCP &middot; {enabledReady.length} server{enabledReady.length !== 1 ? 's' : ''} &middot; {totalTools} tool{totalTools !== 1 ? 's' : ''} &middot; ~{Math.round(totalTools * 250 / 1000)}k tokens
                    {disabledCount > 0 && (
                      <span className="mcp-disabled-count"> ({disabledCount} off)</span>
                    )}
                    {failedServers.length > 0 && (
                      <span className="mcp-failed-count"> &middot; {failedServers.length} failed</span>
                    )}
                    {pendingOAuth.length > 0 && (
                      <span className="mcp-auth-needed"> &middot; auth needed</span>
                    )}
                  </>
          }
        </span>
        {(servers.length > 0 || pendingOAuth.length > 0) && (
          <span
            className={`mcp-panel-reload ${reloading ? 'reloading' : ''}`}
            role="button"
            tabIndex={0}
            onClick={handleReload}
            onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') handleReload(e as unknown as React.MouseEvent); }}
            title="Reload MCP servers"
          >
            &#8635;
          </span>
        )}
      </button>
      {expanded && (
        <div className="mcp-panel-body">
          {servers.length === 0 ? (
            <div className="mcp-empty">No MCP servers connected</div>
          ) : (
            servers.map(server => (
              <div key={server.name} className={`mcp-server-item ${!server.enabled ? 'mcp-server-disabled' : ''}`}>
                <button
                  className="mcp-server-header"
                  onClick={() => toggleServer(server.name)}
                >
                  <span className={`mcp-server-chevron ${expandedServers.has(server.name) ? 'expanded' : ''}`}>
                    &#9654;
                  </span>
                  <span className={`mcp-server-name ${!server.enabled ? 'mcp-name-disabled' : ''}`}>
                    {server.name}
                  </span>
                  <span className="mcp-server-meta">
                    {server.transport}{server.auth !== 'none' ? ` · ${server.auth}` : ''}
                  </span>
                  <span className="mcp-server-count">
                    {server.state === 'failed'
                      ? <span className="mcp-state-failed">failed</span>
                      : server.state === 'unauthorized'
                        ? <span className="mcp-auth-needed">auth needed</span>
                        : `${server.tool_count} tool${server.tool_count !== 1 ? 's' : ''}`}
                  </span>
                  <span
                    className={`mcp-server-toggle ${server.enabled ? 'on' : 'off'} ${togglingServers.has(server.name) ? 'toggling' : ''}`}
                    role="button"
                    tabIndex={0}
                    title={server.enabled ? 'Disable server' : 'Enable server'}
                    onClick={(e) => {
                      e.stopPropagation();
                      if (!togglingServers.has(server.name)) {
                        handleToggleEnabled(server.name, server.enabled);
                      }
                    }}
                    onKeyDown={(e) => {
                      if ((e.key === 'Enter' || e.key === ' ') && !togglingServers.has(server.name)) {
                        e.stopPropagation();
                        handleToggleEnabled(server.name, server.enabled);
                      }
                    }}
                  >
                    {server.enabled ? '\u25CF' : '\u25CB'}
                  </span>
                </button>
                {expandedServers.has(server.name) && (
                  server.state === 'failed' ? (
                    <div className="mcp-server-error">{server.last_error ?? 'connection failed'}</div>
                  ) : (
                    <div className="mcp-tool-list">
                      {server.tools.map(tool => (
                        <span key={tool} className={`mcp-tool-name ${!server.enabled ? 'mcp-tool-disabled' : ''}`}>{tool}</span>
                      ))}
                    </div>
                  )
                )}
              </div>
            ))
          )}
        </div>
      )}
    </div>
  );
}
