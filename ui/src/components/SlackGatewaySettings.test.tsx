import { afterEach, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { SlackGatewaySettings } from './SlackGatewaySettings';

afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

it('offers a conversation link only while the status API confirms native route availability', async () => {
  let conversationAvailable = false;
  let failure: string | null = null;
  vi.stubGlobal('fetch', vi.fn().mockImplementation(async () => ({
    ok: true,
    json: async () => ({
      config: null,
      requests: [{ request_id: 'request', product_id: 'reserved-product', conversation_available: conversationAvailable,
        admitted: false, accepted_at_us: 1, eyes_confirmed: false, failure,
        source: { source_url: 'https://example.slack.com/source', kind: 'briefing', trigger_kind: 'reaction', observed_at_unix_micros: 1, message_ts: '1.000001' } }],
      returns: [],
    }),
  })));
  render(<SlackGatewaySettings standalone />);
  await screen.findByRole('link', { name: 'Slack briefing' });
  expect(screen.queryByRole('link', { name: 'Open Phoenix' })).toBeNull();

  conversationAvailable = true;
  fireEvent.click(screen.getByRole('button', { name: 'Refresh status' }));
  expect((await screen.findByRole('link', { name: 'Open Phoenix' })).getAttribute('href')).toBe('/c/reserved-product');

  conversationAvailable = false;
  failure = 'Conversation unavailable';
  fireEvent.click(screen.getByRole('button', { name: 'Refresh status' }));
  await waitFor(() => expect(screen.queryByRole('link', { name: 'Open Phoenix' })).toBeNull());
  expect(screen.getByText(/Conversation unavailable/)).toBeTruthy();
});

it.each([
  [[], false, false],
  [[{ query: 'owner fire messages', first_page_hits: 1, verified_sources: 0 }, { query: 'fire reactions', first_page_hits: 0, verified_sources: 0 }], false, false],
  [[{ query: 'owner fire messages', first_page_hits: 1, verified_sources: 1 }, { query: 'fire reactions', first_page_hits: 1, verified_sources: 1 }], false, false],
  [[{ query: 'owner fire messages', first_page_hits: 1, verified_sources: 1 }, { query: 'fire reactions', first_page_hits: 1, verified_sources: 1 }], true, true],
])('reports only demonstrated read capabilities', async (checks, reactionReadVerified, demonstrated) => {
  vi.stubGlobal('fetch', vi.fn().mockImplementation(async (url: string) => ({ ok: true, json: async () => url.endsWith('/inspect')
    ? { owner_verified: true, checks, reaction_read_verified: reactionReadVerified, calls: 4 }
    : { config: null, requests: [], returns: [] } })));
  render(<SlackGatewaySettings standalone />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Check Slack read access' }).hasAttribute('disabled')).toBe(false));
  fireEvent.click(screen.getByRole('button', { name: 'Check Slack read access' }));
  const status = await screen.findByRole('status');
  expect(status.textContent).toContain(demonstrated ? 'Sampled source and reaction reads demonstrated' : 'Inconclusive');
  expect(status.textContent).not.toContain('both discovery queries passed');
});

it.each(['prefix', 'reaction', 'unknown_historical'])('only exposes message age for known prefixes: %s', async triggerKind => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true, json: async () => ({ config: null, returns: [], requests: [{ request_id: 'request', product_id: null, conversation_available: false,
    admitted: false, accepted_at_us: 3000000, eyes_confirmed: false, failure: null,
    source: { source_url: 'https://example.slack.com/source', kind: 'explicit', trigger_kind: triggerKind, observed_at_unix_micros: 2000000, message_ts: '1.000001' } }] }) }));
  render(<SlackGatewaySettings standalone />);
  await screen.findByRole('link', { name: 'Slack explicit' });
  expect(screen.getByText(/observed-to-accept 1000 ms/)).toBeTruthy();
  expect(screen.queryByText(/prefix-message age at observation 1000 ms/) !== null).toBe(triggerKind === 'prefix');
});

it('edits the full notification command without losing arguments when replacing its launcher', async () => {
  const command = ['/usr/bin/uv', 'run', '/opt/notifier/notify.py', '--label', 'private notice'];
  const config = { enabled: false, mcp_server: 'slack', owner: 'UOWNER', workspace: 'example.slack.com', timezone: 'UTC', phoenix_url: 'https://phoenix.example', notification_command: command };
  let submitted: typeof config | undefined;
  vi.stubGlobal('fetch', vi.fn().mockImplementation(async (_url: string, options?: RequestInit) => {
    if (options?.method === 'PUT') submitted = JSON.parse(String(options.body));
    return { ok: true, json: async () => ({ config: submitted ?? config, requests: [], returns: [] }) };
  }));
  render(<SlackGatewaySettings standalone />);
  await screen.findByText('Configured');
  fireEvent.click(screen.getByText('Edit notification command'));
  const launcher = screen.getByLabelText('Program or launcher');
  expect((launcher as HTMLInputElement).value).toBe('/usr/bin/uv');
  fireEvent.change(launcher, { target: { value: '' } });
  expect((screen.getByLabelText('Argument 2') as HTMLInputElement).value).toBe('/opt/notifier/notify.py');
  fireEvent.change(launcher, { target: { value: '/opt/bin/uv' } });
  fireEvent.change(screen.getByLabelText('Argument 4'), { target: { value: 'private notice with spaces\nand a newline' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save settings' }));
  await screen.findByText('Settings saved.');
  expect(submitted?.notification_command).toEqual(['/opt/bin/uv', 'run', '/opt/notifier/notify.py', '--label', 'private notice with spaces\nand a newline']);
});

it('keeps best-effort status separate from native work without implying notification delivery', async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true, json: async () => ({ config: null,
    requests: [{ request_id: 'request', product_id: null, conversation_available: false, admitted: true, accepted_at_us: 2, eyes_confirmed: false, eyes_failure: 'Slack acknowledgment unavailable: message_not_found', failure: null,
      source: { source_url: 'https://example.slack.com/source', kind: 'briefing', trigger_kind: 'reaction', observed_at_unix_micros: 1, message_ts: '1.000001' } }],
    returns: [{ return_id: 'notice', state: 'submitted', destination: 'private', request_ids: ['request'], failure: null, notification_receipt: 'receipt' }],
  }) }));
  render(<SlackGatewaySettings standalone />);
  await screen.findByText('Slack acknowledgment unavailable: message_not_found');
  expect(screen.getByText('Admitted')).toBeTruthy();
  expect(screen.getByText(/👀 unconfirmed/)).toBeTruthy();
  expect(screen.queryByText(/Next delivery check after/)).toBeNull();
  expect(screen.getByText('submitted · delivery unchecked')).toBeTruthy();
  expect(screen.getByText('Submission receipt')).toBeTruthy();
  expect(screen.queryByText('Failed')).toBeNull();
});
