import { describe, expect, it } from 'vitest';
import { inputOriginPresentation } from './inputOriginPresentation';

const instance = 'a48ea5c2-37cc-42d5-a1d7-fac1d7765d4e';

describe('inputOriginPresentation', () => {
  it.each(['remote_conversation', 'remote_coordinator'] as const)('qualifies %s without peer configuration', (kind) => {
    const origin = {
      kind, instance_id: instance, product_conversation_id: 'colliding-product',
      transcript_id: 'colliding-transcript', source_call: { message_id: 'remote-message', tool_use_id: 'remote-tool' },
    };
    const presentation = inputOriginPresentation(origin);
    expect(presentation.className).toBe('meta');
    expect(presentation.label).toContain(instance);
    expect(presentation.label).toContain(kind === 'remote_coordinator' ? 'remote Coordinator' : 'remote conversation');
    expect(presentation.label).toContain('colliding-product');
    expect(presentation.label).toContain('colliding-transcript');
    expect(presentation.title).toContain('remote-message');
    expect(presentation.title).toContain('remote-tool');
    expect(inputOriginPresentation({ ...origin, source_call: null }).title).toContain('source call unavailable');
  });

  it('keeps remote API input on the API channel without inventing a conversation', () => {
    const presentation = inputOriginPresentation({ kind: 'remote_api', instance_id: instance });
    expect(presentation.className).toBe('meta');
    expect(presentation.label).toBe(`API · instance ${instance}`);
    expect(presentation.title).toBe(`API-channel input from authenticated instance ${instance}`);
    expect(presentation.label).not.toMatch(/conversation|Coordinator|transcript|User/);
  });

  it('preserves local channel and historical presentation', () => {
    expect(inputOriginPresentation({ kind: 'user_api' }).className).toBe('user');
    expect(inputOriginPresentation({ kind: 'unknown_historical' }).label).toBe('Unknown input');
    expect(inputOriginPresentation(undefined).className).toBe('meta');
  });
});
