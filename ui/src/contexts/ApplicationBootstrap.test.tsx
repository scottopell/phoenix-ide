import { act, cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApplicationBootstrap } from './ApplicationBootstrap';
import { useApplicationCapabilities } from './ApplicationCapabilitiesContext';
import { persistedCapabilities } from '../testing/persistedObservationFixture';
import { ApplicationCapabilitiesSchema } from '../observationSchemas';
import * as v from 'valibot';

function Consumer() {
  const capabilities = useApplicationCapabilities();
  return <div data-testid="capabilities">{JSON.stringify(capabilities)}</div>;
}
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

describe('ApplicationBootstrap', () => {
  it('does not mount consumers until the real capabilities API has been decoded', async () => {
    let finish!: (response: Response) => void;
    const fetchMock = vi.fn(() => new Promise<Response>((resolve) => { finish = resolve; }));
    vi.stubGlobal('fetch', fetchMock);
    render(<ApplicationBootstrap><Consumer /></ApplicationBootstrap>);
    expect(screen.getByRole('status')).toHaveTextContent('Loading application capabilities');
    expect(screen.queryByTestId('capabilities')).toBeNull();
    expect(fetchMock).toHaveBeenCalledWith('/api/capabilities');
    await act(async () => { finish(Response.json(persistedCapabilities)); });
    expect(JSON.parse(screen.getByTestId('capabilities').textContent!)).toEqual(persistedCapabilities);
  });

  it.each([
    ['HTTP failure', () => Promise.resolve(new Response('', { status: 503 }))],
    ['network failure', () => Promise.reject(new Error('offline'))],
    ['malformed capabilities', () => Promise.resolve(Response.json({ live_stream: 'available' }))],
  ])('fails closed on %s rather than supplying live defaults', async (_name, response) => {
    vi.stubGlobal('fetch', vi.fn(response));
    render(<ApplicationBootstrap><Consumer /></ApplicationBootstrap>);
    expect(await screen.findByRole('alert')).toHaveTextContent('Unable to load application capabilities');
    expect(screen.queryByTestId('capabilities')).toBeNull();
  });

  it('requires an explicit capability owner outside bootstrap', () => {
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});
    try { expect(() => render(<Consumer />)).toThrow('Application capabilities require'); }
    finally { consoleError.mockRestore(); }
  });

  it('matches the generated backend capability shape without synthetic fields', () => {
    expect(v.parse(ApplicationCapabilitiesSchema, persistedCapabilities)).toEqual(persistedCapabilities);
  });
});
