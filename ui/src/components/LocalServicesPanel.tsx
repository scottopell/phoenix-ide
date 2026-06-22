import { useCallback, useEffect, useMemo, useState } from 'react';
import { api } from '../api';
import type { DiscoveredService, ServiceCapability } from '../api';

const STORAGE_KEY = 'phoenix:local-services-expanded';

export function LocalServicesPanel() {
  const [services, setServices] = useState<DiscoveredService[]>([]);
  const [localAccess, setLocalAccess] = useState(false);
  const [expanded, setExpanded] = useState(() => {
    try {
      return localStorage.getItem(STORAGE_KEY) === 'true';
    } catch {
      return false;
    }
  });

  const refresh = useCallback(() => {
    api.getLocalServices()
      .then((response) => setServices(response.services))
      .catch(() => setServices([]));
  }, []);

  useEffect(() => {
    api.deploymentInfo()
      .then((info) => setLocalAccess(info.local_access))
      .catch(() => setLocalAccess(false));
  }, []);

  useEffect(() => {
    refresh();
    const id = window.setInterval(refresh, 15_000);
    return () => window.clearInterval(id);
  }, [refresh]);

  const visibleServices = useMemo(
    () => services.filter((service) => service.status === 'healthy' || service.status === 'stale'),
    [services],
  );

  const toggleExpanded = useCallback(() => {
    setExpanded((prev) => {
      const next = !prev;
      try {
        localStorage.setItem(STORAGE_KEY, String(next));
      } catch {
        // Storage disabled; expansion remains in-memory for this render tree.
      }
      return next;
    });
  }, []);

  if (visibleServices.length === 0) return null;

  return (
    <section className="local-services" aria-label="Local services discovered on the Phoenix host">
      <button className="local-services-header" onClick={toggleExpanded} aria-expanded={expanded}>
        <span>Local Services</span>
        <span className="local-services-count">{visibleServices.length}</span>
      </button>
      {expanded && (
        <div className="local-services-list">
          {visibleServices.map((service) => (
            <LocalServiceRow key={service.id} service={service} localAccess={localAccess} />
          ))}
        </div>
      )}
    </section>
  );
}

function LocalServiceRow({ service, localAccess }: { service: DiscoveredService; localAccess: boolean }) {
  const label = service.title || hostLabel(service.base_url) || `:${service.port}`;
  const capabilityText = summarizeCapabilities(service.capabilities);
  const preferredUrl = preferredOpenUrl(service);

  const copyUrl = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(service.base_url);
    } catch {
      // Clipboard can be unavailable on non-secure origins; keep the panel quiet.
    }
  }, [service.base_url]);

  return (
    <div className={`local-service-row ${service.status === 'stale' ? 'stale' : ''}`}>
      <div className="local-service-main">
        <span className="local-service-status" title={service.status === 'healthy' ? 'Healthy' : 'Stale'}>
          {service.status === 'healthy' ? '✓' : '…'}
        </span>
        <span className="local-service-name" title={service.base_url}>{label}</span>
        <span className="local-service-port">:{service.port}</span>
      </div>
      <div className="local-service-meta">
        <span>{capabilityText}</span>
        <span className="local-service-actions">
          <button className="local-service-action" type="button" onClick={copyUrl} title="Copy service URL">Copy</button>
          {localAccess && preferredUrl && (
            <a className="local-service-action" href={preferredUrl} target="_blank" rel="noreferrer" title="Open service link">Open</a>
          )}
        </span>
      </div>
    </div>
  );
}

function summarizeCapabilities(capabilities: ServiceCapability[]): string {
  const labels = new Set<string>();
  for (const capability of capabilities) {
    switch (capability.kind) {
      case 'open_api':
        labels.add('OpenAPI');
        break;
      case 'documentation':
        labels.add('Docs');
        break;
      case 'html_ui':
        labels.add('UI');
        break;
      default:
        break;
    }
  }
  return labels.size > 0 ? Array.from(labels).join(' · ') : 'API catalog';
}

function preferredOpenUrl(service: DiscoveredService): string | null {
  const preferred = service.capabilities.find((capability) =>
    capability.kind === 'html_ui' || capability.kind === 'documentation' || capability.kind === 'open_api'
  );
  if (!preferred) return null;
  return preferred.url;
}

function hostLabel(baseUrl: string): string | null {
  try {
    const url = new URL(baseUrl);
    return url.hostname || null;
  } catch {
    return null;
  }
}
