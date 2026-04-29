import { useState, useEffect, useCallback, useRef } from 'react';
import { api } from '../api';
import type { McpServerStatus } from '../api';
import './McpStatusPanel.css';

interface McpStatusPanelProps {
  showToast: (message: string, duration?: number) => void;
}

export function McpStatusPanel({ showToast }: McpStatusPanelProps) {
  const [servers, setServers] = useState<McpServerStatus[]>([]);
  const [expanded, setExpanded] = useState(false);
  const [expandedServers, setExpandedServers] = useState<Set<string>>(new Set());
  const [reloading, setReloading] = useState(false);
  const [togglingServers, setTogglingServers] = useState<Set<string>>(new Set());
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const fetchStatus = useCallback(async () => {
    try {
      const status = await api.getMcpStatus();
      setServers(status);
      return status.length;
    } catch {
      return 0;
    }
  }, []);

  // Poll every 3s until servers are connected. Keep polling while any server
  // has a pending OAuth URL so the UI can update when auth completes.
  useEffect(() => {
    let cancelled = false;
    const shouldStopPolling = (s: McpServerStatus[]) =>
      s.length > 0 && s.every(srv => !srv.pending_oauth_url);

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

  // Stop polling once all OAuth flows have resolved.
  useEffect(() => {
    if (servers.length > 0 && servers.every(s => !s.pending_oauth_url) && pollRef.current) {
      clearInterval(pollRef.current);
      pollRef.current = null;
    }
  }, [servers]);

  const handleReload = useCallback(async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (reloading) return;
    setReloading(true);
    // Ensure polling is active — connection happens as a background task on the
    // server, so the new OAuth URL won't be in the status we fetch immediately.
    if (!pollRef.current) {
      pollRef.current = setInterval(() => { void fetchStatus(); }, 3000);
    }
    try {
      const result = await api.reloadMcp();
      await fetchStatus();
      const parts: string[] = [];
      if (result.added.length > 0) parts.push(`+${result.added.length} added`);
      if (result.removed.length > 0) parts.push(`-${result.removed.length} removed`);
      if (result.unchanged.length > 0) parts.push(`${result.unchanged.length} unchanged`);
      showToast(`MCP reload: ${parts.join(', ') || 'no servers'}`, 3000);
      // Keep reloading=true until the next poll shows new content (effect below)
      // or the safety timeout fires.
    } catch {
      showToast('MCP reload failed', 3000);
      setReloading(false);
    }
  }, [reloading, fetchStatus, showToast]);

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
      showToast(`Failed to ${currentlyEnabled ? 'disable' : 'enable'} ${serverName}`, 3000);
    } finally {
      setTogglingServers(prev => {
        const next = new Set(prev);
        next.delete(serverName);
        return next;
      });
    }
  }, [fetchStatus, showToast]);

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

  const enabledServers = servers.filter(s => s.enabled);
  const totalTools = enabledServers.reduce((sum, s) => sum + s.tool_count, 0);
  const disabledCount = servers.length - enabledServers.length;

  const pendingOAuth = servers.filter(s => s.pending_oauth_url);

  if (servers.length === 0 && pendingOAuth.length === 0 && !expanded && !reloading) {
    return null;
  }

  return (
    <div className="mcp-panel">
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
        </div>
      ))}
      <button className="mcp-panel-header" onClick={() => setExpanded(!expanded)}>
        <span className={`mcp-panel-chevron ${expanded ? 'expanded' : ''}`}>&#9654;</span>
        <span className="mcp-panel-summary">
          {pendingOAuth.length > 0 && enabledServers.length === 0
            ? <span className="mcp-auth-needed">MCP &middot; auth needed</span>
            : servers.length === 0
              ? 'No MCP servers'
              : <>
                  MCP &middot; {enabledServers.length} server{enabledServers.length !== 1 ? 's' : ''} &middot; {totalTools} tool{totalTools !== 1 ? 's' : ''} &middot; ~{Math.round(totalTools * 250 / 1000)}k tokens
                  {disabledCount > 0 && (
                    <span className="mcp-disabled-count"> ({disabledCount} off)</span>
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
                  <span className="mcp-server-count">
                    {server.tool_count} tool{server.tool_count !== 1 ? 's' : ''}
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
                  <div className="mcp-tool-list">
                    {server.tools.map(tool => (
                      <span key={tool} className={`mcp-tool-name ${!server.enabled ? 'mcp-tool-disabled' : ''}`}>{tool}</span>
                    ))}
                  </div>
                )}
              </div>
            ))
          )}
        </div>
      )}
    </div>
  );
}
