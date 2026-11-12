import { afterEach, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { SlackGatewaySettings } from './SlackGatewaySettings';

afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

it('saves one custom emoji name and explains all three workflows', async () => {
  const config = { enabled: true, mcp_server: 'slack', owner: 'UOWNER', workspace: 'example.slack.com', timezone: 'UTC', phoenix_url: 'https://phoenix.example', trigger_emoji: 'fire', notification_command: null };
  let submitted: typeof config | undefined;
  vi.stubGlobal('fetch', vi.fn().mockImplementation(async (_url: string, options?: RequestInit) => {
    if (options?.method === 'PUT') submitted = JSON.parse(String(options.body));
    return { ok: true, json: async () => ({ config: submitted ?? config, scan: { baseline_complete: !submitted, status: submitted ? 'initializing' : 'healthy' }, requests: [], returns: [] }) };
  }));
  render(<SlackGatewaySettings standalone />);
  const first = await screen.findByLabelText('Slack emoji name');
  await waitFor(() => expect((first as HTMLInputElement).value).toBe('fire'));
  fireEvent.change(first, { target: { value: ':phoenix-please-look:' } });
  expect(screen.queryByRole('button', { name: 'Add trigger emoji' })).toBeNull();
  expect(screen.getByText('1. Message prefix')).toBeTruthy();
  expect(screen.getByText('2. React to your own message')).toBeTruthy();
  expect(screen.getByText('3. React to someone else’s message')).toBeTruthy();
  expect(screen.getByText(':phoenix-please-look: phoenix, investigate this')).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Save settings' }));
  await screen.findByText('Settings saved.');
  expect(submitted?.trigger_emoji).toBe('phoenix-please-look');
  expect(submitted).not.toHaveProperty('trigger_emojis');
  expect(screen.getByText('Initializing')).toBeTruthy();
  expect(screen.getByText(/Changing the emoji restarts initialization/)).toBeTruthy();
  fireEvent.change(first, { target: { value: 'fire' } });
  expect(screen.queryByText('Settings saved.')).toBeNull();
});

it.each(['fire, phoenix-please-look', 'fire phoenix-please-look', 'eyes'])('rejects multiple or reserved names without submitting: %s', async name => {
  const config = { enabled: false, mcp_server: 'slack', owner: 'UOWNER', workspace: 'example.slack.com', timezone: 'UTC', phoenix_url: 'https://phoenix.example', trigger_emoji: 'phoenix-please-look', notification_command: null };
  const fetch = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ config, requests: [], returns: [] }) });
  vi.stubGlobal('fetch', fetch);
  render(<SlackGatewaySettings standalone />);
  await waitFor(() => expect((screen.getByLabelText('Slack emoji name') as HTMLInputElement).value).toBe('phoenix-please-look'));
  fireEvent.change(screen.getByLabelText('Slack emoji name'), { target: { value: name } });
  fireEvent.click(screen.getByRole('button', { name: 'Save settings' }));
  await screen.findByText(name === 'eyes' ? 'eyes is reserved for Phoenix acknowledgments.' : 'Enter one Slack emoji name, such as phoenix-please-look.');
  expect(fetch.mock.calls.some(([, options]) => options?.method === 'PUT')).toBe(false);
});

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

it.each(['prefix', 'reaction'])('only exposes message age for known prefixes: %s', async triggerKind => {
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


it.each([
  [null, 'Connection unavailable'],
  [{ state: 'recovering', enabled: true, tools: [], last_error: 'HTTP 503', next_retry_at: 2000000000 }, 'recovering'],
  [{ state: 'unauthorized', enabled: true, tools: [] }, 'Sign-in required'],
  [{ state: 'ready', enabled: true, tools: [] }, 'Required tools unavailable'],
])('does not report old healthy scans as a healthy current connection', async (connection, label) => {
  const config = { enabled: true, mcp_server: 'slack', owner: 'UOWNER', workspace: 'example.slack.com', timezone: 'UTC', phoenix_url: 'https://phoenix.example', notification_command: [] };
  vi.stubGlobal('fetch', vi.fn().mockImplementation(async (url: string) => ({ ok: true, json: async () => url.endsWith('/capabilities')
    ? { catalogs: [{ server: 'slack', current: true }], tools: [] }
    : { config, connection, stale_scan_warning: true, scan: { baseline_complete: true, status: 'healthy' }, requests: [], returns: [] } })));
  render(<SlackGatewaySettings standalone />);
  await screen.findByText(new RegExp('Slack connection: ' + label));
  expect(screen.queryByText('healthy')).toBeNull();
  expect(screen.getByText(/Older triggers may have been missed/)).toBeTruthy();
});

it.each([true, false])('offers the owned sign-in link only for an enabled unauthorized connection: %s', async enabled => {
  const url = 'https://auth.example/authorize?state=owned-flow';
  const config = { enabled: true, mcp_server: 'slack' };
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true, json: async () => ({ config, connection: { state: 'unauthorized', enabled, tools: [], pending_oauth_url: url }, requests: [], returns: [] }) }));
  render(<SlackGatewaySettings standalone />);
  await screen.findByText(/Slack connection:/);
  const link = screen.queryByRole('link', { name: 'Sign in to Slack' });
  expect(link?.getAttribute('href') ?? null).toBe(enabled ? url : null);
});


it('shows schema incompatibility even when every required tool name is present', async () => {
  const names = ['slack_read_user_profile', 'slack_search_public_and_private', 'slack_read_thread', 'slack_get_reactions', 'slack_add_reaction', 'slack_send_message'];
  const config = { enabled: true, mcp_server: 'slack', owner: 'UOWNER', workspace: 'example.slack.com', timezone: 'UTC', phoenix_url: 'https://phoenix.example', notification_command: [] };
  vi.stubGlobal('fetch', vi.fn().mockImplementation(async (url: string) => ({ ok: true, json: async () => url.endsWith('/capabilities')
    ? { catalogs: [{ server: 'slack', current: true }], tools: names.map(name => ({ server: 'slack', name, input_schema: name === 'slack_send_message' ? { type: 'object', properties: { message: { type: 'number' } } } : { type: 'object' } })) }
    : { config, connection: { name: 'slack', state: 'ready', enabled: true, tools: names }, scan: { baseline_complete: true, status: 'healthy' }, requests: [], returns: [] } })));
  render(<SlackGatewaySettings standalone />);
  await screen.findByText(/Slack connection: Incompatible tool schema: slack_send_message/);
  expect(screen.queryByText('healthy')).toBeNull();
});

it.each([[], ['slack_read_user_profile'], ['slack_read_user_profile', 'slack_search_public_and_private', 'slack_read_thread', 'slack_get_reactions', 'slack_add_reaction', 'slack_send_message']].map(names => ({ names })))('keeps a stale catalog unverified regardless of its previous names: $names', async ({ names }) => {
  const config = { enabled: true, mcp_server: 'slack' };
  vi.stubGlobal('fetch', vi.fn().mockImplementation(async (url: string) => ({ ok: true, json: async () => url.endsWith('/capabilities')
    ? { catalogs: [{ server: 'slack', current: false }], tools: [] }
    : { config, connection: { name: 'slack', state: 'ready', enabled: true, tools: names }, scan: { baseline_complete: true, status: 'healthy' }, requests: [], returns: [] } })));
  render(<SlackGatewaySettings standalone />);
  await screen.findByText(/Slack connection: Cannot verify required tool schemas/);
  expect(screen.queryByText(/Required tools unavailable/)).toBeNull();
  expect(screen.queryByText('healthy')).toBeNull();
});
