import { describe, expect, it } from 'vitest';
import { assertTouchStreamOwner } from './streamOwner';
const base = 'http://127.0.0.1:4179/src/fixtures/touchReading/index.html';
const live = 'touch-reading-member';
describe('touch stream route ownership', () => {
  it.each([
    '/api/conversation-reads/stream?reference=touch-reading-member',
    '/api/conversation-reads/stream?reference=touch-reading-member&after_event_sequence=3&transcript_generation=1&open_id=open',
    'http://127.0.0.1:4179/api/conversation-reads/stream?reference=touch-reading-member',
  ])('accepts production canonical route %s', url => {
    expect(() => assertTouchStreamOwner(url, base, live)).not.toThrow();
  });
  it('uses URLSearchParams decoding exactly once', () => {
    expect(() => assertTouchStreamOwner('/api/conversation-reads/stream?reference=a%2Fb%20c', base, 'a/b c')).not.toThrow();
    expect(() => assertTouchStreamOwner('/api/conversation-reads/stream?reference=a%252Fb', base, 'a/b')).toThrow();
  });
  it.each([
    '/api/conversation-reads/stream',
    '/api/conversation-reads/stream?reference=',
    '/api/conversation-reads/stream?reference=wrong',
    '/api/conversation-reads/stream?reference=touch-reading-member&reference=wrong',
    '/api/conversation-reads/stream?reference=touch-reading-member&reference=touch-reading-member',
    '/api/conversation-reads/other?reference=touch-reading-member',
    '/api/conversations/touch-reading-member/stream',
    '/api/conversation-reads/stream/?reference=touch-reading-member',
    'http://elsewhere.invalid/api/conversation-reads/stream?reference=touch-reading-member',
  ])('rejects wrong endpoint or owner %s', url => {
    expect(() => assertTouchStreamOwner(url, base, live)).toThrow('Unexpected touch fixture stream owner');
  });
  it('rejects absent authoritative identity', () => {
    expect(() => assertTouchStreamOwner('/api/conversation-reads/stream?reference=', base, '')).toThrow();
  });
});
