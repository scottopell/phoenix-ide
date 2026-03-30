import { useState, useEffect, useCallback } from 'react';
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

  const fetchStatus = useCallback(async () => {
    try {
      const status = await api.getMcpStatus();
      setServers(status);
    } catch {
      // Silently fail -- panel just shows nothing
    }
  }, []);

  useEffect(() => {
    fetchStatus();
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
      <div className="mcp-panel-header" onClick={() => setExpanded(!expanded)}>
        <span className={`mcp-panel-chevron ${expanded ? 'expanded' : ''}`}>&#9654;</span>
        <span className="mcp-panel-summary">
          {servers.length === 0
            ? 'No MCP servers'
            : `MCP \u00b7 ${servers.length} server${servers.length !== 1 ? 's' : ''} \u00b7 ${totalTools} tool${totalTools !== 1 ? 's' : ''}`}
        </span>
        {servers.length > 0 && (
          <button
            className={`mcp-panel-reload ${reloading ? 'reloading' : ''}`}
            onClick={handleReload}
            title="Reload MCP servers"
          >
            &#8635;
          </button>
        )}
      </div>
      {expanded && (
        <div className="mcp-panel-body">
          {servers.length === 0 ? (
            <div className="mcp-empty">No MCP servers connected</div>
          ) : (
            servers.map(server => (
              <div key={server.name} className="mcp-server-item">
                <div
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
                </div>
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
