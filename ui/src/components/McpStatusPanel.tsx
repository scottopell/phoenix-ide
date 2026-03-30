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

  // Fetch on mount, then poll every 3s until servers appear (background
  // discovery may still be running). Stop once we have results.
  useEffect(() => {
    let cancelled = false;
    fetchStatus().then(count => {
      if (cancelled || count > 0) return;
      pollRef.current = setInterval(async () => {
        const n = await fetchStatus();
        if (n > 0 && pollRef.current) {
          clearInterval(pollRef.current);
          pollRef.current = null;
        }
      }, 3000);
    });
    return () => {
      cancelled = true;
      if (pollRef.current) clearInterval(pollRef.current);
    };
  }, [fetchStatus]);

  const handleReload = useCallback(async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (reloading) return;
    setReloading(true);
    try {
      const result = await api.reloadMcp();
      await fetchStatus();
      const parts: string[] = [];
      if (result.added.length > 0) parts.push(`+${result.added.length} added`);
      if (result.removed.length > 0) parts.push(`-${result.removed.length} removed`);
      if (result.unchanged.length > 0) parts.push(`${result.unchanged.length} unchanged`);
      showToast(`MCP reload: ${parts.join(', ') || 'no servers'}`, 3000);
    } catch {
      showToast('MCP reload failed', 3000);
    } finally {
      setReloading(false);
    }
  }, [reloading, fetchStatus, showToast]);

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

  if (servers.length === 0 && !expanded) {
    return null;
  }

  return (
    <div className="mcp-panel">
      <button className="mcp-panel-header" onClick={() => setExpanded(!expanded)}>
        <span className={`mcp-panel-chevron ${expanded ? 'expanded' : ''}`}>&#9654;</span>
        <span className="mcp-panel-summary">
          {servers.length === 0
            ? 'No MCP servers'
            : <>
                MCP &middot; {servers.length} server{servers.length !== 1 ? 's' : ''} &middot; {totalTools} tool{totalTools !== 1 ? 's' : ''} &middot; ~{Math.round(totalTools * 250 / 1000)}k tokens
                {disabledCount > 0 && (
                  <span className="mcp-disabled-count"> ({disabledCount} off)</span>
                )}
              </>
          }
        </span>
        {servers.length > 0 && (
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
