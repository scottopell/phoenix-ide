import { describe, expect, it, vi } from 'vitest';
import { observeViewResponse } from './conversationViewResponse';

const trace = '1234567890abcdef1234567890abcdef';
describe('content-free response metadata', () => {
  it.each([
    [null, 'unknown'], ['-1', 'unknown'], ['1e6', 'unknown'], ['secret', 'unknown'],
    ['1000000000000', 'unknown'], ['0', '0-10k'], ['10000', '0-10k'],
    ['10001', '10k-100k'], ['100001', '100k-1m'], ['1000001', '1m+'],
  ])('projects Content-Length %s into %s without reading a response body', (length, bucket) => {
    const response = new Response('Private transcript', { headers: length === null ? {} : { 'content-length': length } });
    const body = vi.spyOn(response, 'text');
    const json = vi.spyOn(response, 'json');
    const observer = vi.fn();
    observeViewResponse(response, observer);
    expect(observer).toHaveBeenCalledWith({ size_bucket: bucket, trace_id: null });
    expect(body).not.toHaveBeenCalled();
    expect(json).not.toHaveBeenCalled();
    expect(response.bodyUsed).toBe(false);
  });
  it('extracts only a valid opaque trace ID and omits all other response attributes', () => {
    const observer = vi.fn();
    observeViewResponse(new Response('Private', { headers: {
      traceparent: `00-${trace}-1234567890abcdef-01`, 'private-title': 'Secret title',
    } }), observer);
    expect(observer).toHaveBeenCalledWith({ size_bucket: 'unknown', trace_id: trace });
    expect(JSON.stringify(observer.mock.calls)).not.toMatch(/Secret|Private|title/);
  });
  it.each(['secret', `00-${'0'.repeat(32)}-1234567890abcdef-01`, `00-${trace}-0000000000000000-01`])('rejects invalid traceparent %s', traceparent => {
    const observer = vi.fn();
    observeViewResponse(new Response(null, { headers: { traceparent } }), observer);
    expect(observer).toHaveBeenCalledWith({ size_bucket: 'unknown', trace_id: null });
  });
  it('contains observer exceptions and never leaks their message', () => {
    const debug = vi.spyOn(console, 'debug').mockImplementation(() => {});
    expect(() => observeViewResponse(new Response(null), () => { throw new Error('Private token'); })).not.toThrow();
    expect(JSON.stringify(debug.mock.calls)).not.toContain('Private token');
    debug.mockRestore();
  });
});
