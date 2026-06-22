import { render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import type { DiscoveredService } from '../api';

const { apiMock } = vi.hoisted(() => ({
  apiMock: {
    deploymentInfo: vi.fn(),
    getLocalServices: vi.fn(),
  },
}));

vi.mock('../api', async () => {
  const actual = await vi.importActual<typeof import('../api')>('../api');
  return {
    ...actual,
    api: apiMock,
  };
});

import { LocalServicesPanel } from './LocalServicesPanel';

const service: DiscoveredService = {
  id: 'loopback:8787',
  base_url: 'http://127.0.0.1:8787/',
  host: '127.0.0.1',
  port: 8787,
  title: 'debug-router',
  description: null,
  capabilities: [
    { kind: 'html_ui', url: 'http://127.0.0.1:8787/', title: 'UI' },
  ],
  first_seen_at: '2026-06-22T00:00:00Z',
  last_seen_at: '2026-06-22T00:00:00Z',
  status: 'healthy',
  confidence: 'explicit_api_catalog',
  source: 'loopback_probe',
};

const serviceWithOnlyOtherLink: DiscoveredService = {
  ...service,
  id: 'loopback:8788',
  port: 8788,
  capabilities: [
    { kind: 'other_link', rel: 'item', url: 'http://127.0.0.1:8788/item', title: 'Item', content_type: null },
  ],
};

describe('LocalServicesPanel', () => {
  beforeEach(() => {
    localStorage.setItem('phoenix:local-services-expanded', 'true');
    apiMock.getLocalServices.mockResolvedValue({ services: [service] });
  });

  afterEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
  });

  it('hides open links for remote browsers', async () => {
    apiMock.deploymentInfo.mockResolvedValue({ local_access: false });

    render(<LocalServicesPanel />);

    expect(await screen.findByText('debug-router')).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByText('Open')).not.toBeInTheDocument());
    expect(screen.getByText('Copy')).toBeInTheDocument();
  });

  it('shows open links for same-host browsers', async () => {
    apiMock.deploymentInfo.mockResolvedValue({ local_access: true });

    render(<LocalServicesPanel />);

    const open = await screen.findByText('Open');
    expect(open).toHaveAttribute('href', 'http://127.0.0.1:8787/');
  });

  it('does not open service roots without an advertised openable link', async () => {
    apiMock.deploymentInfo.mockResolvedValue({ local_access: true });
    apiMock.getLocalServices.mockResolvedValue({ services: [serviceWithOnlyOtherLink] });

    render(<LocalServicesPanel />);

    expect(await screen.findByText('debug-router')).toBeInTheDocument();
    expect(screen.queryByText('Open')).not.toBeInTheDocument();
    expect(screen.getByText('Copy')).toBeInTheDocument();
  });
});
