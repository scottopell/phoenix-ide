import { describe, it, expect } from 'vitest';
import { DraftStore, draftReducer } from './DraftStore';

describe('draftReducer', () => {
  describe('set_draft', () => {
    it('replaces the draft with the new text', () => {
      const atom = { fencedSendRecoveries: [], draft: 'old' };
      const next = draftReducer(atom, { type: 'set_draft', text: 'new' });
      expect(next.draft).toBe('new');
    });

    it('returns the same atom when text is unchanged (no spurious renders)', () => {
      const atom = { fencedSendRecoveries: [], draft: 'same' };
      const next = draftReducer(atom, { type: 'set_draft', text: 'same' });
      expect(next).toBe(atom);
    });

    it('handles empty text as a valid draft value', () => {
      const atom = { fencedSendRecoveries: [], draft: 'old' };
      const next = draftReducer(atom, { type: 'set_draft', text: '' });
      expect(next.draft).toBe('');
    });
  });

  describe('set_draft_if_empty', () => {
    it('sets the draft when current draft is empty', () => {
      const atom = { fencedSendRecoveries: [], draft: '' };
      const next = draftReducer(atom, { type: 'set_draft_if_empty', text: 'seed' });
      expect(next.draft).toBe('seed');
    });

    it('does not replace existing visible content', () => {
      const atom = { fencedSendRecoveries: [], draft: 'existing' };
      const next = draftReducer(atom, { type: 'set_draft_if_empty', text: 'seed' });
      expect(next).toBe(atom);
    });

    it('sets the draft when current draft is whitespace-only', () => {
      const atom = { fencedSendRecoveries: [], draft: '   ' };
      const next = draftReducer(atom, { type: 'set_draft_if_empty', text: 'seed' });
      expect(next.draft).toBe('seed');
    });
  });

  describe('append_draft', () => {
    it('inserts a blank-line separator when existing draft has visible content', () => {
      const atom = { fencedSendRecoveries: [], draft: 'first thought' };
      const next = draftReducer(atom, { type: 'append_draft', text: 'follow-up' });
      expect(next.draft).toBe('first thought\n\nfollow-up');
    });

    it('replaces (no separator) when existing draft is empty', () => {
      const atom = { fencedSendRecoveries: [], draft: '' };
      const next = draftReducer(atom, { type: 'append_draft', text: 'first content' });
      expect(next.draft).toBe('first content');
    });

    it('preserves whitespace-only draft text when appending', () => {
      const atom = { fencedSendRecoveries: [], draft: '   \n  ' };
      const next = draftReducer(atom, { type: 'append_draft', text: 'first content' });
      expect(next.draft).toBe('   \n  \n\nfirst content');
    });

    it('is a no-op when the appended text is empty', () => {
      const atom = { fencedSendRecoveries: [], draft: 'kept' };
      const next = draftReducer(atom, { type: 'append_draft', text: '' });
      expect(next).toBe(atom);
    });
  });

  describe('fenced send recoveries', () => {
    it('queues submissions without flattening their attachment groups', () => {
      const atom = { fencedSendRecoveries: [], draft: '' };
      const first = {
        text: 'first', restoreTo: 'draft' as const, images: [],
        files: [{ original_name: 'first.txt', media_type: 'text/plain', size_bytes: 1, stored_path: '/first' }],
      };
      const second = {
        text: 'second', restoreTo: 'draft' as const, images: [],
        files: [{ original_name: 'second.txt', media_type: 'text/plain', size_bytes: 1, stored_path: '/second' }],
      };
      const queued = draftReducer(
        draftReducer(atom, { type: 'enqueue_fenced_send_recovery', recovery: first }),
        { type: 'enqueue_fenced_send_recovery', recovery: second },
      );

      expect(queued.fencedSendRecoveries).toEqual([first, second]);
      expect(draftReducer(queued, { type: 'shift_fenced_send_recovery' }).fencedSendRecoveries)
        .toEqual([second]);
    });
  });

  describe('clear_draft', () => {
    it('empties the draft', () => {
      const atom = { fencedSendRecoveries: [], draft: 'something' };
      const next = draftReducer(atom, { type: 'clear_draft' });
      expect(next.draft).toBe('');
    });

    it('returns the same atom when draft is already empty', () => {
      const atom = { fencedSendRecoveries: [], draft: '' };
      const next = draftReducer(atom, { type: 'clear_draft' });
      expect(next).toBe(atom);
    });
  });
});

describe('DraftStore', () => {
  it('routes dispatches by slug — different slugs do not interfere', () => {
    const store = new DraftStore();
    store.dispatch('alpha', { type: 'set_draft', text: 'A' });
    store.dispatch('beta', { type: 'set_draft', text: 'B' });
    expect(store.getSnapshot('alpha').draft).toBe('A');
    expect(store.getSnapshot('beta').draft).toBe('B');
  });

  it('notifies only the dispatched slug', () => {
    const store = new DraftStore();
    let alphaTicks = 0;
    let betaTicks = 0;
    store.subscribe('alpha', () => alphaTicks++);
    store.subscribe('beta', () => betaTicks++);
    store.dispatch('alpha', { type: 'set_draft', text: 'A' });
    expect(alphaTicks).toBe(1);
    expect(betaTicks).toBe(0);
  });

  it('preserves snapshot reference identity on no-op dispatches', () => {
    const store = new DraftStore();
    store.dispatch('alpha', { type: 'set_draft', text: 'A' });
    const before = store.getSnapshot('alpha');
    store.dispatch('alpha', { type: 'set_draft', text: 'A' });
    expect(store.getSnapshot('alpha')).toBe(before);
  });
});
