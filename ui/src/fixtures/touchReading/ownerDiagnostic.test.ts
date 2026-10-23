import { describe, expect, it } from 'vitest';
import { assertTouchStreamOwner } from './streamOwner';
const base = 'http://127.0.0.1:4179/fixture';
const live = 'touch-reading-member';
function rejection(url: string, expected = live) {
  try { assertTouchStreamOwner(url, base, expected); }
  catch (error) { return (error as Error).message; }
  throw new Error('Expected rejection');
}
describe('bounded stream-owner rejection diagnostics', () => {
  it('identifies the known refresh channel without accepting it', () => {
    const message = rejection('/api/product-conversations/events');
    expect(message).toContain('"reason":"wrong-channel"');
    expect(message).toContain('"path":"/api/product-conversations/events"');
    expect(message).toContain('"referenceCount":0');
  });
  it('never includes foreign origin, unknown path, unknown key or query values', () => {
    const message = rejection('https://SECRET_HOST.invalid/SECRET_PATH?SECRET_KEY=SECRET_VALUE&reference=SECRET_REF&open_id=SECRET_OPEN');
    for (const secret of ['SECRET_HOST', 'SECRET_PATH', 'SECRET_KEY', 'SECRET_VALUE', 'SECRET_REF', 'SECRET_OPEN']) {
      expect(message).not.toContain(secret);
    }
    expect(message).toContain('"sameOrigin":false');
    expect(message).toContain('"path":"other"');
    expect(message).toContain('"queryKeys":["other","reference","open_id"]');
  });
  it('does not disclose an unknown expected owner', () => {
    expect(rejection('/api/conversation-reads/stream?reference=wrong', 'SECRET_EXPECTED')).not.toContain('SECRET_EXPECTED');
    expect(rejection('/api/conversation-reads/stream?reference=wrong', 'SECRET_EXPECTED')).toContain('"expectedSyntheticOwner":"other"');
  });
  it('reports duplicate matching owners as invalid', () => {
    const message = rejection(`/api/conversation-reads/stream?reference=${live}&reference=${live}`);
    expect(message).toContain('"reason":"reference-count"');
    expect(message).toContain('"referenceCount":2');
    expect(message).toContain('"referenceMatches":false');
  });
  it('bounds output independent of arbitrary key/value lengths', () => {
    const query = Array.from({ length: 20 }, (_, i) => `SECRET_${i}_${'x'.repeat(1000)}=${'y'.repeat(1000)}`).join('&');
    const message = rejection(`/unknown?${query}`);
    expect(message).not.toContain('SECRET');
    expect(message.length).toBeLessThan(512);
    const data = JSON.parse(message.slice(message.indexOf('{')));
    expect(data.queryKeys).toHaveLength(8);
    expect(data.queryKeys.every((key: string) => key === 'other')).toBe(true);
  });
  it('preserves valid conversation acceptance', () => {
    expect(() => assertTouchStreamOwner(`/api/conversation-reads/stream?reference=${live}`, base, live)).not.toThrow();
  });
});
