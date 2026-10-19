import { describe, it, expect, vi } from 'vitest';
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

    it('sets the draft when current draft is whitespace-only', () => {
      const atom = { draft: '   ' };
      const next = draftReducer(atom, { type: 'set_draft_if_empty', text: 'seed' });
      expect(next.draft).toBe('seed');
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

    it('preserves whitespace-only draft text when appending', () => {
      const atom = { draft: '   \n  ' };
      const next = draftReducer(atom, { type: 'append_draft', text: 'first content' });
      expect(next.draft).toBe('   \n  \n\nfirst content');
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
  it('conditionally restores only the exact empty revision, including ABA and attachment-only sends', () => {
    const store = new DraftStore();
    const recovery = store.recovery('a');
    const first = recovery.clearForSubmission();
    expect(recovery.restoreIfUnchanged(first, 'raw rejected')).toBe(true);
    const second = recovery.clearForSubmission();
    store.dispatch('a', { type: 'append_draft', text: 'external insertion' });
    store.dispatch('a', { type: 'clear_draft' });
    expect(recovery.restoreIfUnchanged(second, 'must not restore')).toBe(false);
    const third = recovery.clearForSubmission();
    const fourth = recovery.clearForSubmission();
    expect(third).not.toBe(fourth);
    expect(recovery.restoreIfUnchanged(third, 'older attachment-only send')).toBe(false);
    expect(recovery.restoreIfUnchanged(fourth, 'latest')).toBe(true);
  });

  it('retains rejected payloads separately by original slug without changing the text revision', () => {
    const store = new DraftStore();
    const recovery = store.recovery('a');
    const revision = recovery.clearForSubmission();
    const payload = { id: 'one', text: '  raw\n', images: [{ media_type: 'image/png', data: 'YWJj' }], files: [], error: 'missing' };
    recovery.retainRejected(payload);
    expect(recovery.readRevision()).toBe(revision);
    expect(store.getRejected('a')).toEqual([payload]);
    expect(store.getRejected('b')).toEqual([]);
    recovery.removeRejected('one');
    expect(store.getRejected('a')).toEqual([]);
  });

  it('invalidates revisions and drops rejected payloads on removal', () => {
    const store = new DraftStore();
    const recovery = store.recovery('a');
    const revision = recovery.clearForSubmission();
    recovery.retainRejected({ id: 'one', text: 'raw', images: [], files: [], error: 'missing' });
    store.remove('a');
    expect(recovery.restoreIfUnchanged(revision, 'stale')).toBe(false);
    expect(store.getRejected('a')).toEqual([]);
  });

  it('makes every old recovery operation inert after removal without recreating an atom', () => {
    const store = new DraftStore();
    const recovery = store.recovery('a');
    const revision = recovery.clearForSubmission();
    store.remove('a');
    const snapshot = vi.spyOn(store, 'getSnapshot');
    const notified = vi.fn();
    store.subscribe('a', notified);

    const invalid = recovery.readRevision();
    expect(recovery.clearForSubmission()).toBe(invalid);
    expect(recovery.restoreIfUnchanged(revision, 'stale')).toBe(false);
    expect(recovery.restoreIfUnchanged(invalid, 'also stale')).toBe(false);
    recovery.retainRejected({ id: 'one', text: 'deleted', images: [], files: [], error: 'missing' });
    recovery.removeRejected('one');

    expect(snapshot).not.toHaveBeenCalled();
    expect(notified).not.toHaveBeenCalled();
    expect(store.getRejected('a')).toEqual([]);
  });

  it('keeps old handles inert after a fresh lifetime reuses the same slug', () => {
    const store = new DraftStore();
    const old = store.recovery('a');
    expect(store.recovery('a')).toBe(old);
    const oldRevision = old.clearForSubmission();
    store.remove('a');
    const fresh = store.recovery('a');
    expect(fresh).not.toBe(old);
    expect(store.recovery('a')).toBe(fresh);
    const freshRevision = fresh.clearForSubmission();
    const payload = { id: 'one', text: 'fresh', images: [], files: [], error: 'missing' };
    fresh.retainRejected(payload);
    const notified = vi.fn();
    store.subscribe('a', notified);

    expect(old.restoreIfUnchanged(oldRevision, 'deleted')).toBe(false);
    expect(old.restoreIfUnchanged(freshRevision, 'deleted')).toBe(false);
    old.retainRejected({ ...payload, text: 'deleted' });
    old.removeRejected('one');
    old.clearForSubmission();
    expect(old.readRevision()).not.toBe(freshRevision);

    expect(notified).not.toHaveBeenCalled();
    expect(fresh.readRevision()).toBe(freshRevision);
    expect(store.getRejected('a')).toEqual([payload]);
    expect(fresh.restoreIfUnchanged(freshRevision, 'new conversation')).toBe(true);
    old.clearForSubmission();
    expect(store.getSnapshot('a').draft).toBe('new conversation');
    fresh.removeRejected('one');
    expect(store.getRejected('a')).toEqual([]);
  });

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
