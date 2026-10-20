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
