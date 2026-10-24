import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { FederationConnections } from './FederationConnections';

const peer = { peer_instance_id: 'peer-id', peer_display_name: 'Laptop', base_url: 'https://laptop.example/', created_at: '2026-01-01T00:00:00Z' };

function mockFetch(saveFails = false) {
  const fetchMock = vi.fn().mockImplementation((path: string, init?: RequestInit) => {
    if (init?.method === 'PUT') return Promise.resolve(new Response(JSON.stringify({ notes: JSON.parse(init.body as string).notes }), { status: saveFails ? 503 : 200 }));
    return Promise.resolve(new Response(JSON.stringify(path.endsWith('/notes') ? { notes: '  observations\n雪  ' } : [peer])));
  });
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

async function openEditor() {
  fireEvent.click(screen.getByRole('button', { name: /Connections/ }));
  const select = await screen.findByRole('combobox', { name: 'Connection' });
  await screen.findByRole('option', { name: /Laptop/ });
  fireEvent.change(select, { target: { value: peer.peer_instance_id } });
  return screen.findByRole('textbox', { name: 'Notes for Laptop' });
}

afterEach(() => vi.unstubAllGlobals());

describe('FederationConnections', () => {
  it('imports one pasted enrollment bundle, disables while busy, then refreshes and selects the peer', async () => {
    const enrollment = {
      receiver_instance_id: peer.peer_instance_id,
      receiver_display_name: peer.peer_display_name,
      receiver_base_url: peer.base_url,
      caller_instance_id: 'caller-id',
      token: 'phx_peer_secret-token',
      tls_trust: { kind: 'platform_roots' },
    };
    let peerLoads = 0;
    let resolveImport!: (response: Response) => void;
    const importResponse = new Promise<Response>((resolve) => { resolveImport = resolve; });
    const fetchMock = vi.fn().mockImplementation((path: string, init?: RequestInit) => {
      if (path === '/api/federation/peers/import') return importResponse;
      if (path === '/api/federation/peers') {
        peerLoads++;
        return Promise.resolve(new Response(JSON.stringify(peerLoads === 1 ? [] : [peer])));
      }
      if (path.endsWith('/notes')) return Promise.resolve(new Response(JSON.stringify({ notes: '' })));
      throw new Error(`Unexpected request: ${path} ${init?.method ?? 'GET'}`);
    });
    vi.stubGlobal('fetch', fetchMock);
    render(<FederationConnections />);
    fireEvent.click(screen.getByRole('button', { name: /Connections/ }));
    await screen.findByText('No saved peer connections.');

    const input = screen.getByRole('textbox', { name: 'Receiver-issued enrollment bundle' });
    expect(screen.queryByRole('textbox', { name: /name|url/i })).not.toBeInTheDocument();
    fireEvent.change(input, { target: { value: JSON.stringify(enrollment) } });
    fireEvent.click(screen.getByRole('button', { name: 'Import connection' }));
    expect(input).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Importing…' })).toBeDisabled();

    const post = fetchMock.mock.calls.find(([path]) => path === '/api/federation/peers/import');
    if (!post) throw new Error('Expected import request');
    expect(post[1]).toMatchObject({ method: 'POST', headers: { 'Content-Type': 'application/json' } });
    expect(JSON.parse(post[1].body as string)).toEqual({ enrollment });
    resolveImport(new Response(JSON.stringify({ peer_instance_id: peer.peer_instance_id })));

    expect(await screen.findByRole('status')).toHaveTextContent('Connection imported and selected.');
    expect(input).toHaveValue('');
    expect(peerLoads).toBe(2);
    expect(screen.getByRole('combobox', { name: 'Connection' })).toHaveValue(peer.peer_instance_id);
    expect(await screen.findByRole('textbox', { name: 'Notes for Laptop' })).toBeVisible();
  });

  it('keeps enrollment failures generic and does not submit malformed JSON', async () => {
    const fetchMock = vi.fn().mockImplementation((path: string) => {
      if (path === '/api/federation/peers/import') return Promise.resolve(new Response('secret remote diagnostic', { status: 502 }));
      return Promise.resolve(new Response(JSON.stringify([])));
    });
    vi.stubGlobal('fetch', fetchMock);
    render(<FederationConnections />);
    fireEvent.click(screen.getByRole('button', { name: /Connections/ }));
    await screen.findByText('No saved peer connections.');
    const input = screen.getByRole('textbox', { name: 'Receiver-issued enrollment bundle' });

    fireEvent.change(input, { target: { value: '{not json' } });
    fireEvent.click(screen.getByRole('button', { name: 'Import connection' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Could not import connection');
    expect(fetchMock).toHaveBeenCalledTimes(1);

    fireEvent.change(input, { target: { value: '{"token":"not echoed"}' } });
    fireEvent.click(screen.getByRole('button', { name: 'Import connection' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Could not import connection');
    expect(screen.queryByText('secret remote diagnostic')).not.toBeInTheDocument();
    expect(input).toHaveValue('{"token":"not echoed"}');
    expect(screen.getByRole('button', { name: 'Import connection' })).toBeEnabled();
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it('loads safe metadata lazily and saves only explicit whole-text edits, including empty', async () => {
    const fetchMock = mockFetch();
    render(<FederationConnections />);
    expect(fetchMock).not.toHaveBeenCalled();
    const editor = await openEditor();
    expect(editor).toHaveValue('  observations\n雪  ');
    expect(screen.getByText(peer.base_url)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
    const text = '  do not interpret {"sender":"coordinator"}\n雪  ';
    fireEvent.change(editor, { target: { value: text } });
    expect(fetchMock).toHaveBeenCalledTimes(2);
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(screen.getByText(/UTF-8 bytes ✓/)).toBeInTheDocument());
    const firstSave = fetchMock.mock.calls[2];
    if (!firstSave) throw new Error('Expected first save request');
    expect(JSON.parse(firstSave[1].body)).toEqual({ notes: text });
    fireEvent.change(editor, { target: { value: '' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(4));
    const emptySave = fetchMock.mock.calls[3];
    if (!emptySave) throw new Error('Expected empty save request');
    expect(JSON.parse(emptySave[1].body)).toEqual({ notes: '' });
  });

  it('bounds UTF-8 bytes and retains unsaved text after save failure', async () => {
    const fetchMock = mockFetch(true);
    render(<FederationConnections />);
    const editor = await openEditor();
    fireEvent.change(editor, { target: { value: '雪'.repeat(10923) } });
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
    expect(screen.getByRole('alert')).toHaveTextContent('32 KiB');
    fireEvent.change(editor, { target: { value: 'retained edit' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await screen.findByText(/Could not save notes/);
    expect(editor).toHaveValue('retained edit');
    expect(screen.getByRole('button', { name: 'Save' })).toBeEnabled();
    expect(fetchMock).toHaveBeenCalledTimes(3);
  });

  it('replaces the notes loading state with a bounded error', async () => {
    vi.stubGlobal('fetch', vi.fn().mockImplementation((path: string) => Promise.resolve(
      new Response(path.endsWith('/notes') ? 'secret-notes-diagnostic' : JSON.stringify([peer]), {
        status: path.endsWith('/notes') ? 500 : 200,
      }),
    )));
    render(<FederationConnections />);
    fireEvent.click(screen.getByRole('button', { name: /Connections/ }));
    const select = await screen.findByRole('combobox', { name: 'Connection' });
    fireEvent.change(select, { target: { value: peer.peer_instance_id } });
    expect(await screen.findByRole('alert')).toHaveTextContent('Could not load notes');
    expect(screen.queryByText('Loading notes…')).not.toBeInTheDocument();
    expect(screen.queryByText('secret-notes-diagnostic')).not.toBeInTheDocument();
  });

  it('shows load errors without rendering remote response text', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('secret-server-diagnostic', { status: 500 })));
    render(<FederationConnections />);
    fireEvent.click(screen.getByRole('button', { name: /Connections/ }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Could not load connections');
    expect(screen.queryByText('secret-server-diagnostic')).not.toBeInTheDocument();
  });
});
