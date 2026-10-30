import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { api, ApiResponseError, type ProductConversationCreationObservation } from './api';

describe('ProductConversation creation client', () => {
  const fetchMock = vi.fn<typeof fetch>();
  beforeEach(() => { fetchMock.mockReset(); vi.stubGlobal('fetch', fetchMock); });
  afterEach(() => { vi.unstubAllGlobals(); });

  it('returns the 202 acceptance identity without requiring a canonical route', async () => {
    const request = { request_id: 'request-1', cwd: '/repo', objective: 'Start work', model: 'model-1' };
    const accepted = { request_id: 'request-1', product_conversation_id: 'product-1' };
    fetchMock.mockResolvedValue(new Response(JSON.stringify(accepted), { status: 202 }));

    await expect(api.createProductConversation(request)).resolves.toEqual(accepted);
    expect(fetchMock).toHaveBeenCalledWith('/api/product-conversations/new', {
      method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(request),
    });
  });

  const identityAndStage = {
    request_id: 'req/1', product_conversation_id: 'product-1',
    provisioning_stage: 'resolve_starting_pin', stage_updated_at: '2026-01-01T00:00:00Z',
  } as const;
  const observations: ProductConversationCreationObservation[] = [
    ...(['accepted', 'claimed', 'retry_scheduled', 'cancelling', 'deletion_pending', 'delivery_pending'] as const)
      .map((status): ProductConversationCreationObservation => ({
        ...identityAndStage, kind: 'pending', status,
        retry_at: status === 'retry_scheduled' ? '2026-01-01T00:00:02Z' : null,
        allowed_actions: ['cancel', 'delete'],
      })),
    ...(['failed', 'cleanup_ambiguous', 'delivery_failed'] as const)
      .map((status): ProductConversationCreationObservation => ({
        ...identityAndStage, kind: 'failed', status,
        error: 'Unable to finish creation', allowed_actions: status === 'delivery_failed' ? ['retry_delivery'] : ['start_over', 'delete'],
      })),
    { ...identityAndStage, kind: 'cancelled', error: null, allowed_actions: ['start_over', 'delete'] },
    { kind: 'published', request_id: 'req/1', product_conversation_id: 'product-1', transcript_row_id: 'transcript-1', canonical_route: '/c/product-1' },
    { kind: 'historical_published', request_id: 'req/1', product_conversation_id: 'product-1', transcript_row_id: 'transcript-1' },
  ];

  it.each(observations)('GETs the exact request and retains its $kind result', async (observation) => {
    const controller = new AbortController();
    fetchMock.mockResolvedValue(new Response(JSON.stringify(observation), { status: 200 }));
    await expect(api.getProductConversationCreation('req/1', controller.signal)).resolves.toEqual(observation);
    expect(fetchMock).toHaveBeenCalledWith('/api/product-conversations/creation/req%2F1', {
      signal: controller.signal, cache: 'no-store',
    });
  });

  it('surfaces an exact-request read failure instead of falling back to the recovery list', async () => {
    fetchMock.mockResolvedValue(new Response(JSON.stringify({ error: 'Request not found' }), { status: 404 }));
    await expect(api.getProductConversationCreation('unknown')).rejects.toMatchObject({
      name: 'ApiResponseError', status: 404, message: 'Request not found',
    });
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it.each([400, 500, 503])('distinguishes a received HTTP %s rejection from an uncertain transport outcome', async (status) => {
    fetchMock.mockResolvedValue(new Response(JSON.stringify({ error: 'Admission failed' }), { status }));
    await expect(api.createProductConversation({ request_id: 'r', cwd: '', objective: 'Work', model: 'model-1' }))
      .rejects.toMatchObject({ name: 'ApiResponseError', status, message: 'Admission failed' });
  });

  it('retains the HTTP failure status when a server error body is not JSON', async () => {
    fetchMock.mockResolvedValue(new Response('Unavailable', { status: 502 }));
    await expect(api.createProductConversation({ request_id: 'r', cwd: '/repo', objective: 'Work', model: 'model-1' }))
      .rejects.toBeInstanceOf(ApiResponseError);
  });
});
