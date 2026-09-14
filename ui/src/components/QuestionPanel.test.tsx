import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { api, QuestionMutationError, type UserQuestion } from '../api';
import { QuestionPanel } from './QuestionPanel';

const question: UserQuestion = { header: 'Scope', question: 'Where to search?', multiSelect: false, options: [{label: 'Current', preview: 'current preview'}, {label: 'Family'}] };
const defaults = { questions: [question], conversationId: 'conv', toolUseId: 'request-1', showToast: vi.fn(), onAnswered: vi.fn(), onDismissed: vi.fn(), onResolved: vi.fn() };
const deferred = () => { let resolve!: (value: {success:boolean}) => void; let reject!: (error: Error) => void; const promise = new Promise<{success:boolean}>((yes,no) => { resolve=yes; reject=no; }); return {promise,resolve,reject}; };
afterEach(() => { vi.restoreAllMocks(); vi.clearAllMocks(); });

describe('QuestionPanel request and draft contract', () => {
  it('starts unanswered, derives preview from selection, and includes collapsed notes', async () => {
    const send = vi.spyOn(api, 'respondToQuestion').mockResolvedValue({success:true});
    render(<QuestionPanel {...defaults} />);
    expect(screen.getByRole('button', {name:'Send answer'})).toBeDisabled();
    expect(screen.getByText('Choose an option to view its preview.')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('radio', {name:'Current'}));
    expect(screen.getByText('current preview')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('radio', {name:'Family'}));
    expect(screen.queryByText('current preview')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', {name:'Add notes (optional)'}));
    fireEvent.change(screen.getByLabelText('Notes for the agent'), {target:{value:'Keep this note'}});
    fireEvent.click(screen.getByRole('button', {name:'Edit notes · included'}));
    fireEvent.click(screen.getByRole('button', {name:'Send answer'}));
    await waitFor(() => expect(send).toHaveBeenCalledWith('conv','request-1',{'Where to search?':'Family'}, {'Where to search?':{notes:'Keep this note'}}));
  });
  it('retains Other across selection/navigation and excludes deselected custom drafts', () => {
    render(<QuestionPanel {...defaults} questions={[question, {...question, question:'Second?', header:'Second'}]} />);
    fireEvent.click(screen.getByRole('radio', {name:'Other'}));
    fireEvent.change(screen.getByLabelText('Custom answer'), {target:{value:'Custom scope'}});
    fireEvent.click(screen.getByRole('radio', {name:'Current'}));
    expect(screen.getByText(/Saved custom draft/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', {name:'Next'}));
    fireEvent.click(screen.getByRole('button', {name:'Back'}));
    fireEvent.click(screen.getByRole('button', {name:'Edit custom answer'}));
    expect(screen.getByLabelText('Custom answer')).toHaveValue('Custom scope');
  });
  it('does not toggle multiselect Other when its editor is clicked; empty Other invalidates form', () => {
    render(<QuestionPanel {...defaults} questions={[{...question,multiSelect:true}]} />);
    fireEvent.click(screen.getByRole('checkbox', {name:'Current'}));
    fireEvent.click(screen.getByRole('checkbox', {name:'Other'}));
    expect(screen.getByRole('button', {name:'Send answer'})).toBeDisabled();
    fireEvent.click(screen.getByLabelText('Custom answer'));
    expect(screen.getByRole('checkbox', {name:'Other'})).toBeChecked();
    fireEvent.change(screen.getByLabelText('Custom answer'), {target:{value:'Custom'}});
    expect(screen.getByRole('button', {name:'Send answer'})).toBeEnabled();
    const other = screen.getByRole('checkbox', {name:'Other'});
    other.focus(); fireEvent.click(other, {detail:1});
    expect(other).not.toBeChecked(); expect(other).toHaveFocus();
    expect(screen.getByText(/Custom answer not included/)).toBeInTheDocument();
  });
  it('requires dismissal confirmation and leaves drafts intact on cancel', () => {
    const dismiss = vi.spyOn(api,'dismissQuestion').mockResolvedValue({success:true});
    render(<QuestionPanel {...defaults} />);
    fireEvent.click(screen.getByRole('button',{name:'Use chat instead'}));
    expect(screen.getByRole('dialog')).toHaveTextContent('The agent will wait for your message.');
    expect(dismiss).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button',{name:'Keep answering'}));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });
  it('allows editing after an initial proven rejection', async () => {
    vi.spyOn(api,'respondToQuestion').mockRejectedValue(new QuestionMutationError('Rejected','question_request_invalid'));
    render(<QuestionPanel {...defaults} />);
    fireEvent.click(screen.getByRole('radio',{name:'Current'}));
    fireEvent.click(screen.getByRole('button',{name:'Send answer'}));
    await screen.findByText('Rejected');
    expect(screen.getByRole('radio',{name:'Family'})).toBeEnabled();
  });
  it('freezes an uncertain operation and retries the identical snapshot even after a later rejection', async () => {
    const send = vi.spyOn(api,'respondToQuestion').mockRejectedValueOnce(new Error('network')).mockRejectedValue(new QuestionMutationError('Stale','question_request_stale'));
    vi.spyOn(api,'getConversation').mockResolvedValue({conversation:{state:{type:'awaiting_user_response',tool_use_id:'request-1',questions:[question]}}} as Awaited<ReturnType<typeof api.getConversation>>);
    render(<QuestionPanel {...defaults} />);
    fireEvent.click(screen.getByRole('radio',{name:'Current'}));
    fireEvent.click(screen.getByRole('button',{name:'Send answer'}));
    fireEvent.click(await screen.findByRole('button',{name:'Retry same answer'}));
    await waitFor(() => expect(send).toHaveBeenCalledTimes(2));
    await screen.findByRole('button',{name:'Retry same answer'});
    expect(send.mock.calls[1]).toEqual(send.mock.calls[0]);
    expect(screen.getByRole('radio',{name:'Family'})).toBeDisabled();
  });
  it('does not apply late success to a newer request with identical questions', async () => {
    const pending = deferred(); vi.spyOn(api,'respondToQuestion').mockReturnValue(pending.promise);
    const {rerender} = render(<QuestionPanel {...defaults} />);
    fireEvent.click(screen.getByRole('radio',{name:'Current'}));
    fireEvent.click(screen.getByRole('button',{name:'Send answer'}));
    rerender(<QuestionPanel {...defaults} toolUseId="request-2" />);
    await act(async () => pending.resolve({success:true}));
    expect(defaults.onAnswered).not.toHaveBeenCalled();
    expect(screen.getByRole('radio',{name:'Current'})).not.toBeChecked();
  });
  it('presents static read-only content', () => {
    render(<QuestionPanel {...defaults} readOnly />);
    expect(screen.queryByRole('radio')).not.toBeInTheDocument();
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
  });
});

describe('AUQ failure and shortcut boundaries', () => {
  it('reconciles a proven stale request without reopening editing', async () => {
    vi.spyOn(api,'respondToQuestion').mockRejectedValue(new QuestionMutationError('Stale','question_request_stale'));
    vi.spyOn(api,'getConversation').mockResolvedValue({conversation:{state:{type:'awaiting_user_response',tool_use_id:'request-2',questions:[question]}}} as Awaited<ReturnType<typeof api.getConversation>>);
    render(<QuestionPanel {...defaults} />);
    fireEvent.click(screen.getByRole('radio',{name:'Current'}));
    fireEvent.click(screen.getByRole('button',{name:'Send answer'}));
    await waitFor(()=>expect(defaults.onResolved).toHaveBeenCalledWith({type:'awaiting_user_response',tool_use_id:'request-2',questions:[question]}));
    expect(screen.getByRole('radio',{name:'Family'})).toBeDisabled();
  });
  it('does not resolve uncertainty from a malformed pending identity', async () => {
    vi.spyOn(api,'respondToQuestion').mockRejectedValue(new Error('network'));
    vi.spyOn(api,'getConversation').mockResolvedValue({conversation:{state:{type:'awaiting_user_response',questions:[question]}}} as Awaited<ReturnType<typeof api.getConversation>>);
    render(<QuestionPanel {...defaults} />);
    fireEvent.click(screen.getByRole('radio',{name:'Current'}));
    fireEvent.click(screen.getByRole('button',{name:'Send answer'}));
    await screen.findByText(/Could not check question status/);
    expect(defaults.onResolved).not.toHaveBeenCalled();
    expect(screen.queryByRole('button',{name:'Retry same answer'})).not.toBeInTheDocument();
  });
  it('ignores composing send shortcuts and blocks overlapping submissions', async () => {
    const pending = deferred(); const send = vi.spyOn(api,'respondToQuestion').mockReturnValue(pending.promise);
    render(<QuestionPanel {...defaults} />);
    const radio=screen.getByRole('radio',{name:'Current'}); fireEvent.click(radio);
    fireEvent.keyDown(radio,{key:'Enter',ctrlKey:true,isComposing:true}); expect(send).not.toHaveBeenCalled();
    fireEvent.keyDown(radio,{key:'Enter',ctrlKey:true}); fireEvent.keyDown(radio,{key:'Enter',ctrlKey:true});
    expect(send).toHaveBeenCalledTimes(1);
    await act(async()=>pending.resolve({success:true}));
  });
  it('hands off a dismissal confirmation to the command palette without dismissing questions', () => {
    const dismiss=vi.spyOn(api,'dismissQuestion'); render(<QuestionPanel {...defaults} />);
    fireEvent.click(screen.getByRole('button',{name:'Use chat instead'}));
    act(()=>window.dispatchEvent(new CustomEvent('command-palette-opening')));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument(); expect(dismiss).not.toHaveBeenCalled();
  });
});
