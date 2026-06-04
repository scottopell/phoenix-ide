// The single-live-stream slot must allow several viewers of the SAME
// sub-agent (an expanded inline card + the docked panel) while still blocking
// a different sub-agent from streaming live concurrently. A bare owner string
// freed the slot when the first viewer closed, even though another viewer's
// EventSource was still open — letting a third agent acquire and produce
// concurrent live streams (Codex review on PR #219). The slot is refcounted.

import { describe, it, expect, beforeEach } from 'vitest';
import { __testing } from './useConversationInlineStream';

const { acquireLiveStream, releaseLiveStream, resetLiveStreamSlot } = __testing;

describe('inline-stream live slot', () => {
  beforeEach(resetLiveStreamSlot);

  it('allows multiple viewers of the same conversation', () => {
    expect(acquireLiveStream('A')).toBe(true); // inline card
    expect(acquireLiveStream('A')).toBe(true); // docked panel, same agent
  });

  it('blocks a different conversation until every viewer of the owner releases', () => {
    expect(acquireLiveStream('A')).toBe(true);
    expect(acquireLiveStream('A')).toBe(true);
    expect(acquireLiveStream('B')).toBe(false); // A owns the slot (count 2)

    releaseLiveStream('A'); // one viewer closes
    expect(acquireLiveStream('B')).toBe(false); // still owned (count 1)

    releaseLiveStream('A'); // last viewer closes
    expect(acquireLiveStream('B')).toBe(true); // slot freed
  });

  it('ignores a release for a conversation that does not own the slot', () => {
    expect(acquireLiveStream('A')).toBe(true);
    releaseLiveStream('B'); // no-op
    expect(acquireLiveStream('B')).toBe(false); // A still owns
  });
});
