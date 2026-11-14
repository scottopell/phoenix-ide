import { describe, expect, it } from 'vitest';
import { AnnotationDraftStore, formatAnnotatedMessage } from './AnnotationDraftStore';

const source = {
  messageId: 'answer', sequenceId: 2, occurrenceToken: 'history:answer', quote: 'The exact passage.',
  textAnchor: { start: { fragmentId: 'text-0', offset: 4 }, end: { fragmentId: 'text-0', offset: 22 } },
};

describe('annotation draft ownership', () => {
  it('edits the comment without changing its source, and removes the whole contribution', () => {
    const draft = new AnnotationDraftStore().forScope('current');
    draft.add({ source, body: 'Original' });
    const id = draft.getSnapshot()[0]!.id;
    draft.edit(id, 'Edited comment');
    expect(draft.getSnapshot()[0]).toMatchObject({ id, source, body: 'Edited comment' });
    expect(draft.getSnapshot()[0]!.source).toBe(source);
    draft.remove(id);
    expect(draft.getSnapshot()).toEqual([]);
  });

  it('never reuses a label after deletion, submission, or edits within a draft scope', () => {
    const draft = new AnnotationDraftStore().forScope('a');
    draft.add({ source, body: 'First' });
    draft.remove(draft.getSnapshot()[0]!.id);
    draft.add({ source, body: 'Second' });
    draft.edit(draft.getSnapshot()[0]!.id, 'Edited');
    expect(draft.getSnapshot()[0]!.number).toBe(2);
    const submission = draft.beginSubmission();
    draft.finishSubmission(submission, true);
    draft.add({ source, body: 'Third' });
    expect(draft.getSnapshot()[0]!.number).toBe(3);
  });

  it('clears only the submitted batch and preserves new annotations and other conversations', () => {
    const store = new AnnotationDraftStore();
    const draft = store.forScope('a');
    const other = store.forScope('b');
    draft.add({ source, body: 'First' });
    other.add({ source, body: 'Other conversation' });
    const submission = draft.beginSubmission();
    const id = submission.annotations[0]!.id;
    draft.edit(id, 'Cannot change in-flight payload');
    draft.remove(id);
    expect(draft.getSnapshot()[0]).toMatchObject({ body: 'First', submission: submission.id });
    draft.add({ source, body: 'Added during send' });
    draft.finishSubmission(submission, true);
    expect(draft.getSnapshot().map(item => item.body)).toEqual(['Added during send']);
    expect(other.getSnapshot()[0]!.body).toBe('Other conversation');
  });

  it('makes a failed batch editable again and ignores stale completion after another attempt', () => {
    const draft = new AnnotationDraftStore().forScope('a');
    draft.add({ source, body: 'First' });
    const first = draft.beginSubmission();
    draft.finishSubmission(first, false);
    draft.edit(first.annotations[0]!.id, 'Corrected');
    const retry = draft.beginSubmission();
    draft.finishSubmission(first, true);
    expect(draft.getSnapshot()[0]).toMatchObject({ body: 'Corrected', submission: retry.id });
    draft.finishSubmission(retry, true);
    expect(draft.getSnapshot()).toEqual([]);
  });

  it('retired capabilities cannot resurrect annotations after hard deletion or mutate a reused scope', () => {
    const store = new AnnotationDraftStore();
    const old = store.forScope('a');
    old.add({ source, body: 'Old' });
    const submission = old.beginSubmission();
    store.remove('a');
    const next = store.forScope('a');
    next.add({ source, body: 'New' });
    old.finishSubmission(submission, false);
    expect(() => old.add({ source, body: 'Late callback' })).toThrow('no longer available');
    expect(() => old.beginSubmission()).toThrow('no longer available');
    expect(old.getSnapshot()).toEqual([]);
    expect(next.getSnapshot().map(item => item.body)).toEqual(['New']);
  });

  it('serializes only at submission and quotes embedded fences without losing text', () => {
    const draft = new AnnotationDraftStore().forScope('a');
    draft.add({ source: { ...source, quote: '```ts\nconst a = 1;\n```' }, body: 'Keep\nthese details.' });
    const text = formatAnnotatedMessage('General feedback', draft.beginSubmission().annotations);
    expect(text).toBe('General feedback\n\nRegarding message #2 (history:answer):\n\n````text\n```ts\nconst a = 1;\n```\n````\n\nKeep\nthese details.');
  });
});
