import { afterEach, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { SlackGatewaySettings } from './SlackGatewaySettings';

afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

it('offers a conversation link only while the status API confirms that product exists', async () => {
  let productExists = false;
  let failure: string | null = null;
  vi.stubGlobal('fetch', vi.fn().mockImplementation(async () => ({
    ok: true,
    json: async () => ({
      config: null,
      requests: [{ request_id: 'request', product_id: 'reserved-product', product_exists: productExists,
        admitted: false, accepted_at_us: 1, eyes_confirmed: false, failure,
        source: { source_url: 'https://example.slack.com/source', kind: 'briefing', observed_at_unix_micros: 1, message_ts: '1.000001' } }],
      returns: [],
    }),
  })));
  render(<SlackGatewaySettings standalone />);
  await screen.findByRole('link', { name: 'Slack briefing' });
  expect(screen.queryByRole('link', { name: 'Open Phoenix' })).toBeNull();

  productExists = true;
  fireEvent.click(screen.getByRole('button', { name: 'Refresh status' }));
  expect((await screen.findByRole('link', { name: 'Open Phoenix' })).getAttribute('href')).toBe('/c/reserved-product');

  productExists = false;
  failure = 'Conversation unavailable';
  fireEvent.click(screen.getByRole('button', { name: 'Refresh status' }));
  await waitFor(() => expect(screen.queryByRole('link', { name: 'Open Phoenix' })).toBeNull());
  expect(screen.getByText(/Conversation unavailable/)).toBeTruthy();
});
