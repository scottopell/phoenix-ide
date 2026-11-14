import { useSyncExternalStore } from 'react';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { AnnotationDraftStore, type AnnotationDraft } from '../conversation/AnnotationDraftStore';
import { DraftAnnotationCards } from './DraftAnnotationCards';

const source = { messageId: 'answer', sequenceId: 2, quote: 'The complete original passage.' };
function Harness({ draft }: { draft: AnnotationDraft }) {
  const annotations = useSyncExternalStore(draft.subscribe, draft.getSnapshot);
  return <><textarea id="message-input" aria-label="Message" /><DraftAnnotationCards draft={draft} annotations={annotations} disabled={false} /></>;
}
function setup() {
  const draft = new AnnotationDraftStore().forScope('a');
  draft.add({ source, body: 'First comment' });
  draft.add({ source, body: 'Second comment' });
  render(<Harness draft={draft} />);
  return draft;
}

describe('annotation stack editor', () => {
  it('edits the selected record in a modal, saves on Escape, and restores focus to the same strip', async () => {
    const draft = setup();
    expect(screen.queryByRole('textbox', { name: /Annotation/ })).not.toBeInTheDocument();
    const second = screen.getByRole('button', { name: 'Edit annotation 2' });
    fireEvent.click(second);
    const editor = screen.getByRole('textbox', { name: 'Annotation 2 comment' });
    expect(editor).toHaveFocus();
    expect(screen.getByRole('dialog')).toHaveTextContent(source.quote);
    fireEvent.change(editor, { target: { value: 'A small tweak' } });
    fireEvent.keyDown(editor, { key: 'Escape' });
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    await waitFor(() => expect(second).toHaveFocus());
    expect(draft.getSnapshot().map(item => item.body)).toEqual(['First comment', 'A small tweak']);
    expect(screen.getAllByRole('button', { name: /Edit annotation/ })).toEqual([screen.getByRole('button', { name: 'Edit annotation 1' }), second]);
    fireEvent.click(second);
    expect(screen.getByRole('textbox', { name: 'Annotation 2 comment' })).toHaveValue('A small tweak');
  });

  it('deletes only the chosen record, preserves numbering, and focuses the next strip', async () => {
    const draft = setup();
    act(() => draft.add({ source, body: 'Third comment' }));
    fireEvent.click(screen.getByRole('button', { name: 'Edit annotation 2' }));
    fireEvent.click(screen.getByRole('button', { name: 'Delete annotation' }));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(draft.getSnapshot().map(item => item.number)).toEqual([1, 3]);
    await waitFor(() => expect(screen.getByRole('button', { name: 'Edit annotation 3' })).toHaveFocus());
    act(() => draft.add({ source, body: 'Fourth comment' }));
    expect(screen.getByRole('button', { name: 'Edit annotation 4' })).toHaveTextContent('Fourth comment');
  });

  it('returns focus to the composer after deleting the last annotation', async () => {
    const draft = new AnnotationDraftStore().forScope('a');
    draft.add({ source, body: 'Only comment' });
    render(<Harness draft={draft} />);
    fireEvent.click(screen.getByRole('button', { name: 'Edit annotation 1' }));
    fireEvent.click(screen.getByRole('button', { name: 'Delete annotation' }));
    expect(screen.queryByRole('region', { name: 'Annotations' })).not.toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole('textbox', { name: 'Message' })).toHaveFocus());
  });

  it('retains the editor on unavailable source, then closes after a successful source return', async () => {
    const draft = setup();
    const navigate = vi.fn().mockResolvedValueOnce(false).mockResolvedValueOnce(true);
    draft.registerNavigator(navigate);
    fireEvent.click(screen.getByRole('button', { name: 'Edit annotation 1' }));
    fireEvent.click(screen.getByRole('button', { name: /Show in conversation/ }));
    await screen.findByText('The source passage is unavailable. Your annotation is retained.');
    expect(screen.getByRole('dialog')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /Show in conversation/ }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(navigate).toHaveBeenLastCalledWith(source, expect.any(AbortSignal));
    expect(draft.getSnapshot()).toHaveLength(2);
  });

  it('closes an inspected in-flight annotation when its batch is accepted', () => {
    const draft = setup();
    let submission!: ReturnType<AnnotationDraft['beginSubmission']>;
    act(() => { submission = draft.beginSubmission(); });
    fireEvent.click(screen.getByRole('button', { name: 'Edit annotation 1' }));
    expect(screen.getByRole('textbox', { name: 'Annotation 1 comment' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Delete annotation' })).toBeDisabled();
    act(() => draft.finishSubmission(submission, true));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });
});
