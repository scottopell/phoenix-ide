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

  const totalTools = servers.reduce((sum, s) => sum + s.tool_count, 0);

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
            : `MCP \u00b7 ${servers.length} server${servers.length !== 1 ? 's' : ''} \u00b7 ${totalTools} tool${totalTools !== 1 ? 's' : ''}`}
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
              <div key={server.name} className="mcp-server-item">
                <button
                  className="mcp-server-header"
                  onClick={() => toggleServer(server.name)}
                >
                  <span className={`mcp-server-chevron ${expandedServers.has(server.name) ? 'expanded' : ''}`}>
                    &#9654;
                  </span>
                  <span className="mcp-server-name">{server.name}</span>
                  <span className="mcp-server-count">
                    {server.tool_count} tool{server.tool_count !== 1 ? 's' : ''}
                  </span>
                </button>
                {expandedServers.has(server.name) && (
                  <div className="mcp-tool-list">
                    {server.tools.map(tool => (
                      <span key={tool} className="mcp-tool-name">{tool}</span>
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
