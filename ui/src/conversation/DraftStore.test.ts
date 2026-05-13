import { describe, it, expect } from 'vitest';
import { DraftStore, draftReducer } from './DraftStore';

describe('draftReducer', () => {
  describe('set_draft', () => {
    it('replaces the draft with the new text', () => {
      const atom = { draft: 'old' };
      const next = draftReducer(atom, { type: 'set_draft', text: 'new' });
      expect(next.draft).toBe('new');
    });

    it('returns the same atom when text is unchanged (no spurious renders)', () => {
      const atom = { draft: 'same' };
      const next = draftReducer(atom, { type: 'set_draft', text: 'same' });
      expect(next).toBe(atom);
    });

    it('handles empty text as a valid draft value', () => {
      const atom = { draft: 'old' };
      const next = draftReducer(atom, { type: 'set_draft', text: '' });
      expect(next.draft).toBe('');
    });
  });

  describe('set_draft_if_empty', () => {
    it('sets the draft when current draft is empty', () => {
      const atom = { draft: '' };
      const next = draftReducer(atom, { type: 'set_draft_if_empty', text: 'seed' });
      expect(next.draft).toBe('seed');
    });

    it('does not replace existing visible content', () => {
      const atom = { draft: 'existing' };
      const next = draftReducer(atom, { type: 'set_draft_if_empty', text: 'seed' });
      expect(next).toBe(atom);
    });

    it('does not replace whitespace-only user input', () => {
      const atom = { draft: '   ' };
      const next = draftReducer(atom, { type: 'set_draft_if_empty', text: 'seed' });
      expect(next).toBe(atom);
    });
  });

  describe('append_draft', () => {
    it('inserts a blank-line separator when existing draft has visible content', () => {
      const atom = { draft: 'first thought' };
      const next = draftReducer(atom, { type: 'append_draft', text: 'follow-up' });
      expect(next.draft).toBe('first thought\n\nfollow-up');
    });

    it('replaces (no separator) when existing draft is empty', () => {
      const atom = { draft: '' };
      const next = draftReducer(atom, { type: 'append_draft', text: 'first content' });
      expect(next.draft).toBe('first content');
    });

    it('replaces (no separator) when existing draft is whitespace-only', () => {
      const atom = { draft: '   \n  ' };
      const next = draftReducer(atom, { type: 'append_draft', text: 'first content' });
      expect(next.draft).toBe('first content');
    });

    it('is a no-op when the appended text is empty', () => {
      const atom = { draft: 'kept' };
      const next = draftReducer(atom, { type: 'append_draft', text: '' });
      expect(next).toBe(atom);
    });
  });

  describe('clear_draft', () => {
    it('empties the draft', () => {
      const atom = { draft: 'something' };
      const next = draftReducer(atom, { type: 'clear_draft' });
      expect(next.draft).toBe('');
    });

    it('returns the same atom when draft is already empty', () => {
      const atom = { draft: '' };
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
