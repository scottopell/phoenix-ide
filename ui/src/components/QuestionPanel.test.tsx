import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { api, QuestionMutationError, type UserQuestion } from '../api';
import { QuestionPanel } from './QuestionPanel';

const question: UserQuestion = { header: 'Scope', question: 'Where to search?', multiSelect: false, options: [{label: 'Current', preview: 'current preview'}, {label: 'Family'}] };
const defaults = { questions: [question], conversationId: 'conv', requestId: 'request-1', showToast: vi.fn(), onResolved: vi.fn() };
const deferred = () => { let resolve!: (value: {success:boolean}) => void; let reject!: (error: Error) => void; const promise = new Promise<{success:boolean}>((yes,no) => { resolve=yes; reject=no; }); return {promise,resolve,reject}; };
beforeEach(() => { vi.spyOn(api,'getConversationStatus').mockResolvedValue({conversation:{state:{type:'idle'}}} as Awaited<ReturnType<typeof api.getConversationStatus>>); });
afterEach(() => { vi.restoreAllMocks(); vi.clearAllMocks(); });

describe('QuestionPanel request and draft contract', () => {
  it('closes open notes before dismissal from another control', () => {
    render(<QuestionPanel {...defaults} />);
    fireEvent.click(screen.getByRole('button', {name:'Add notes (optional)'}));
    const choice = screen.getByRole('radio', {name:'Current'});
    choice.focus();
    fireEvent.keyDown(choice, {key:'Escape'});
    expect(screen.queryByLabelText('Notes for the agent')).not.toBeInTheDocument();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.getByRole('button', {name:'Add notes (optional)'})).toHaveFocus();
  });
  it('normalizes nested REST state before resolving uncertainty', async () => {
    vi.spyOn(api,'respondToQuestion').mockRejectedValue(new Error('network'));
    vi.spyOn(api,'getConversationStatus').mockResolvedValue({conversation:{state:{type:'recoverable_continuation_failure', failure:{message:'Summary failed',error_kind:'server_error',request:{operation_id:'continue-1',attempt:2}}}}} as unknown as Awaited<ReturnType<typeof api.getConversationStatus>>);
    render(<QuestionPanel {...defaults} />);
    fireEvent.click(screen.getByRole('radio',{name:'Current'}));
    fireEvent.click(screen.getByRole('button',{name:'Send answer'}));
    await waitFor(()=>expect(defaults.onResolved).toHaveBeenCalledWith({type:'recoverable_continuation_failure',message:'Summary failed',error_kind:'server_error',operation_id:'continue-1',attempt:2}, null));
  });
  it('renders preview code as a separate block while sending the original preview', async () => {
    const preview = 'Compare this code:\n\n```ts\nconst value = "long code";\n```';
    const send = vi.spyOn(api,'respondToQuestion').mockResolvedValue({success:true});
    const {container} = render(<QuestionPanel {...defaults} questions={[{...question,options:[{label:'Code',preview}]}]} />);
    fireEvent.click(screen.getByRole('radio',{name:'Code'}));
    expect(container.querySelector('.question-preview-text pre code')?.textContent).toBe('const value = "long code";\n');
    const code = container.querySelector('.question-preview-text pre');
    fireEvent.click(screen.getByRole('button',{name:'Add notes (optional)'}));
    expect(container.querySelector('.question-preview-text pre')).toBe(code);
    fireEvent.click(screen.getByRole('button',{name:'Send answer'}));
    await waitFor(()=>expect(send).toHaveBeenCalledWith('conv','request-1',{'Where to search?':'Code'},{'Where to search?':{preview}}));
  });
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

  it('keeps question navigation available in short multi-question layouts', () => {
    const rect = vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({
      x: 0, y: 0, width: 500, height: 240, top: 0, left: 0, right: 500, bottom: 240,
      toJSON: () => ({}),
    } as DOMRect);
    const width = vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockReturnValue(500);
    const many = [question, {...question, question:'Second?', header:'Second'}, {...question, question:'Third?', header:'Third'}, {...question, question:'Fourth?', header:'Fourth'}];
    render(<QuestionPanel {...defaults} questions={many} />);

    expect(screen.getByLabelText('Answer agent questions')).toHaveClass('question-panel--short');
    expect(screen.getByRole('navigation', {name:'Questions'})).toBeInTheDocument();
    expect(screen.getByRole('button', {name:'Second, unanswered'})).toBeEnabled();

    rect.mockRestore();
    width.mockRestore();
  });

  it('restores focus to the preview disclosure when Escape collapses expanded preview', () => {
    const scrollHeight = vi.spyOn(HTMLElement.prototype, 'scrollHeight', 'get').mockReturnValue(400);
    const preview = 'Compare this code:\n\n```ts\nconst value = "long code";\n```';
    const {container} = render(<QuestionPanel {...defaults} questions={[{...question, options:[{label:'Code', preview}]}]} />);
    fireEvent.click(screen.getByRole('radio', {name:'Code'}));
    const disclosure = screen.getByRole('button', {name:'Show full preview'});
    fireEvent.click(disclosure);
    const code = container.querySelector<HTMLElement>('.question-preview-text pre');
    expect(code).toBeTruthy();
    code!.focus();
    fireEvent.keyDown(code!, {key:'Escape'});

    expect(screen.getByRole('button', {name:'Show full preview'})).toHaveFocus();

    scrollHeight.mockRestore();
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
    const send = vi.spyOn(api,'respondToQuestion').mockRejectedValueOnce(new Error('network')).mockRejectedValue(new QuestionMutationError('Rejected','question_request_invalid'));
    vi.spyOn(api,'getConversationStatus').mockResolvedValue({conversation:{state:{type:'awaiting_user_response',request_id: 'request-1', tool_use_id:'request-1',questions:[question]}}} as Awaited<ReturnType<typeof api.getConversationStatus>>);
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
    rerender(<QuestionPanel {...defaults} requestId="request-2" />);
    await act(async () => pending.resolve({success:true}));
    expect(defaults.onResolved).not.toHaveBeenCalled();
    expect(screen.getByRole('radio',{name:'Current'})).not.toBeChecked();
  });
  it('presents static read-only content', () => {
    render(<QuestionPanel {...defaults} readOnly />);
    expect(screen.queryByRole('radio')).not.toBeInTheDocument();
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
  });
});

describe('AUQ failure and shortcut boundaries', () => {
  it('focuses the Other preview from the narrow layout shortcut', () => {
    render(<QuestionPanel {...defaults} />);
    const link = screen.getByRole('button', {name:'Preview below'});
    expect(link).toBeDisabled();
    fireEvent.click(screen.getByRole('radio', {name:'Other'}));
    expect(link).toBeEnabled();
    fireEvent.click(link);
    expect(screen.getByRole('heading', {name:'Preview — Other'})).toHaveFocus();
  });
  it('carries the server phase timestamp after successful answer refresh', async () => {
    vi.spyOn(api,'respondToQuestion').mockResolvedValue({success:true});
    vi.spyOn(api,'getConversationStatus').mockResolvedValue({conversation:{state:{type:'llm_requesting',attempt:1},state_updated_at:'2026-09-14T15:00:00Z'}} as Awaited<ReturnType<typeof api.getConversationStatus>>);
    render(<QuestionPanel {...defaults} />);
    fireEvent.click(screen.getByRole('radio',{name:'Current'}));
    fireEvent.click(screen.getByRole('button',{name:'Send answer'}));
    await waitFor(()=>expect(defaults.onResolved).toHaveBeenCalledWith({type:'llm_requesting',attempt:1}, Date.parse('2026-09-14T15:00:00Z')));
  });
  it('adopts authoritative recovery state after successful answer without SSE', async () => {
    vi.spyOn(api,'respondToQuestion').mockResolvedValue({success:true});
    vi.spyOn(api,'getConversationStatus').mockResolvedValue({conversation:{state:{type:'recoverable_continuation_failure', failure:{message:'Projection failed',error_kind:'server_error',request:{operation_id:'op-1',attempt:1}}}}} as unknown as Awaited<ReturnType<typeof api.getConversationStatus>>);
    render(<QuestionPanel {...defaults} />);
    fireEvent.click(screen.getByRole('radio',{name:'Current'}));
    fireEvent.click(screen.getByRole('button',{name:'Send answer'}));
    await waitFor(()=>expect(defaults.onResolved).toHaveBeenCalledWith({type:'recoverable_continuation_failure',message:'Projection failed',error_kind:'server_error',operation_id:'op-1',attempt:1}, null));
    expect(defaults.showToast).toHaveBeenCalledWith('Answers sent');
  });
  it.each(['success','stale'] as const)('keeps %s closed when refresh fails, then checks without resending', async outcome => {
    const send = vi.spyOn(api,'respondToQuestion');
    if (outcome === 'success') send.mockResolvedValue({success:true});
    else send.mockRejectedValue(new QuestionMutationError('Stale','question_request_stale'));
    vi.spyOn(api,'getConversationStatus').mockRejectedValueOnce(new Error('offline')).mockResolvedValue({conversation:{state:{type:'idle'}}} as Awaited<ReturnType<typeof api.getConversationStatus>>);
    render(<QuestionPanel {...defaults} />);
    fireEvent.click(screen.getByRole('radio',{name:'Current'}));
    fireEvent.click(screen.getByRole('button',{name:'Send answer'}));
    await screen.findByText(/conversation status could not be refreshed/);
    expect(screen.queryByRole('radio')).not.toBeInTheDocument();
    expect(screen.queryByRole('button',{name:'Retry same answer'})).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button',{name:'Check status again'}));
    await waitFor(()=>expect(defaults.onResolved).toHaveBeenCalledWith({type:'idle'}, null));
    expect(send).toHaveBeenCalledTimes(1);
  });
  it('ignores a late successful refresh after a newer request appears', async () => {
    vi.spyOn(api,'respondToQuestion').mockResolvedValue({success:true});
    let resolve!: (value: Awaited<ReturnType<typeof api.getConversationStatus>>) => void;
    const get = vi.spyOn(api,'getConversationStatus').mockReturnValue(new Promise(yes => {resolve=yes;}));
    const {rerender}=render(<QuestionPanel {...defaults} />);
    fireEvent.click(screen.getByRole('radio',{name:'Current'}));
    fireEvent.click(screen.getByRole('button',{name:'Send answer'}));
    await waitFor(()=>expect(get).toHaveBeenCalled());
    rerender(<QuestionPanel {...defaults} requestId="request-2" />);
    await act(async()=>resolve({conversation:{state:{type:'idle'}}} as Awaited<ReturnType<typeof api.getConversationStatus>>));
    expect(defaults.onResolved).not.toHaveBeenCalled();
    expect(screen.getByRole('radio',{name:'Current'})).not.toBeChecked();
  });

  it('reconciles a proven stale request without reopening editing', async () => {
    vi.spyOn(api,'respondToQuestion').mockRejectedValue(new QuestionMutationError('Stale','question_request_stale'));
    vi.spyOn(api,'getConversationStatus').mockResolvedValue({conversation:{state:{type:'awaiting_user_response',request_id: 'request-2', tool_use_id:'request-2',questions:[question]}}} as Awaited<ReturnType<typeof api.getConversationStatus>>);
    render(<QuestionPanel {...defaults} />);
    fireEvent.click(screen.getByRole('radio',{name:'Current'}));
    fireEvent.click(screen.getByRole('button',{name:'Send answer'}));
    await waitFor(()=>expect(defaults.onResolved).toHaveBeenCalledWith({type:'awaiting_user_response',request_id: 'request-2', tool_use_id:'request-2',questions:[question]}, null));
    expect(screen.queryByRole('radio')).not.toBeInTheDocument();
  });
  it.each([
    {type:'awaiting_user_response',questions:[question]},
    {},
    {type:'unrecognized_future_state'},
  ])('does not resolve uncertainty from malformed or unknown state %j', async state => {
    vi.spyOn(api,'respondToQuestion').mockRejectedValue(new Error('network'));
    vi.spyOn(api,'getConversationStatus').mockResolvedValue({conversation:{state}} as Awaited<ReturnType<typeof api.getConversationStatus>>);
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
