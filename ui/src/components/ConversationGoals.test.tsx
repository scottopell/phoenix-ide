import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { MockInstance } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { ConversationGoalDetails, ConversationGoalSummary } from './ConversationGoals';
import { useConversationGoals } from '../hooks/useConversationGoals';
import { GoalApiError, goalApi } from '../goalApi';
import type { GoalEntry } from '../goalApi';

const principal = '22222222-2222-4222-8222-222222222222';
const requestId = '11111111-1111-4111-8111-111111111111';
function fact(owner = 'pc-1', version = 1): GoalEntry {
  const actor = { UserControl: { principal_id: principal, request_id: requestId, command: 'create' as const } };
  return {
    snapshot: { owner, goal_id: 'goal-1', version, objective: `${owner} objective`, state: 'Underway' },
    mutation: { owner, expected_version: version - 1, action_id: `create-${version}`, attribution: { requested_by: actor, applied_by: actor }, change: { Create: { goal_id: 'goal-1', objective: `${owner} objective` } } },
    recorded_at_us: version,
  };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function Harness({ owner = 'pc-1', writable = true }: { owner?: string | null; writable?: boolean }) {
  const model = useConversationGoals(owner, writable);
  return <><ConversationGoalSummary model={model} /><ConversationGoalDetails key={owner} model={model} /></>;
}
let current: MockInstance<typeof goalApi.current>;
let control: MockInstance<typeof goalApi.control>;
let history: MockInstance<typeof goalApi.history>;
let command: MockInstance<typeof goalApi.command>;
beforeEach(() => {
  current = vi.spyOn(goalApi, 'current').mockResolvedValue(null);
  control = vi.spyOn(goalApi, 'control').mockResolvedValue(null);
  history = vi.spyOn(goalApi, 'history').mockResolvedValue([]);
  command = vi.spyOn(goalApi, 'command').mockResolvedValue('Command recorded.');
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); });
async function create(objective = '  preserve this exact objective  ') {
  fireEvent.change(await screen.findByLabelText('New objective'), { target: { value: objective } });
  fireEvent.click(screen.getByRole('button', { name: 'Create goal' }));
}

describe('ordinary conversation goal extension', () => {
  it('reports default uninstalled 503, not no-goal or successful empty facts', async () => {
    current.mockRejectedValue(new GoalApiError('Goals unavailable — services are not installed.', 503, 'known'));
    render(<Harness />);
    expect(await screen.findByText('Goals unavailable — services are not installed.')).toBeInTheDocument();
    expect(screen.queryByText('No recorded goal.')).not.toBeInTheDocument();
    expect(screen.queryByLabelText('New objective')).not.toBeInTheDocument();
    expect(command).not.toHaveBeenCalled();
  });

  it('renders current state/reason/source and History/Close facts without editable fields', async () => {
    const entry = { ...fact(), snapshot: { ...fact().snapshot, state: { Blocked: 'waiting for actual credentials' } } };
    current.mockResolvedValue(entry);
    history.mockResolvedValue([entry]);
    control.mockResolvedValue({ version: 3, stopped: true });
    render(<Harness writable={false} />);
    expect(await screen.findByText('GOAL Blocked · v1 · STOP')).toBeInTheDocument();
    expect(screen.getAllByText('waiting for actual credentials')).toHaveLength(2);
    expect(screen.getAllByText(new RegExp(`Owner ${principal}`))).toHaveLength(2);
    expect(screen.getByText(/Read-only: History or Close/)).toBeInTheDocument();
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Stop goal work' })).not.toBeInTheDocument();
    expect(command).not.toHaveBeenCalled();
  });

  it('creates only on explicit submit and never optimistically changes facts before GET', async () => {
    const receipt = deferred<string>();
    command.mockReturnValueOnce(receipt.promise);
    render(<Harness />);
    await create();
    expect(command).toHaveBeenCalledTimes(1);
    expect(command.mock.calls[0]!).toEqual(['pc-1', { kind: 'create', body: { request_id: expect.any(String), expected_version: 0, objective: '  preserve this exact objective  ' } }]);
    expect(screen.getByText('No recorded goal.')).toBeInTheDocument();
    current.mockResolvedValue(fact());
    await act(async () => { receipt.resolve('Command recorded.'); await receipt.promise; });
    expect(await screen.findByText('GOAL Underway · v1')).toBeInTheDocument();
    expect(current).toHaveBeenCalledTimes(2);
  });

  it('retries unknown network/corrupt-receipt outcomes with exact UUID/payload even after refresh fails', async () => {
    command.mockRejectedValueOnce(new GoalApiError('Invalid receipt: outcome unknown.', 200, 'unknown'));
    render(<Harness />);
    await create();
    const retry = await screen.findByRole('button', { name: 'Retry exact command' });
    const original = command.mock.calls[0]!;
    expect(screen.getByRole('button', { name: 'Create goal' })).toBeDisabled();
    current.mockRejectedValueOnce(new Error('refresh failed'));
    fireEvent.click(screen.getByRole('button', { name: 'Refresh goals' }));
    expect(await screen.findByText('refresh failed')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Retry exact command' })).toBeEnabled();
    fireEvent.click(retry);
    await waitFor(() => expect(command).toHaveBeenCalledTimes(2));
    expect(command.mock.calls[1]!).toEqual(original);
    expect(command.mock.calls[1]![1]).toBe(original[1]);
  });

  it.each([403, 503])('keeps the original unknown command when an exact retry is rejected before mutation (%s)', async (status) => {
    command.mockRejectedValueOnce(new GoalApiError('lost original response', null, 'unknown'))
      .mockRejectedValueOnce(new GoalApiError('retry rejected before mutation', status, 'known'));
    const view = render(<Harness />);
    await create();
    const original = command.mock.calls[0]!;
    fireEvent.click(await screen.findByRole('button', { name: 'Retry exact command' }));
    expect(await screen.findByText('retry rejected before mutation')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Create goal' })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'Refresh goals' }));
    expect(await screen.findByText('No recorded goal.')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Create goal' })).toBeDisabled();
    view.rerender(<Harness owner="pc-2" />);
    await waitFor(() => expect(screen.getByRole('button', { name: 'Create goal' })).toBeEnabled());
    view.rerender(<Harness owner="pc-1" />);
    fireEvent.click(await screen.findByRole('button', { name: 'Retry exact command' }));
    await waitFor(() => expect(command).toHaveBeenCalledTimes(3));
    expect(command.mock.calls[1]!).toEqual(original);
    expect(command.mock.calls[2]!).toEqual(original);
    expect(command.mock.calls[2]![1]).toBe(original[1]);
  });

  it('fences stale reads and mutation completions across owners and retains the exact retry on switchback', async () => {
    const lateRead = deferred<GoalEntry | null>();
    const lateReceipt = deferred<string>();
    current.mockImplementation((owner) => owner === 'pc-2' ? lateRead.promise : Promise.resolve(null));
    command.mockReturnValueOnce(lateReceipt.promise);
    const view = render(<Harness />);
    await create();
    const original = command.mock.calls[0]!;
    view.rerender(<Harness owner="pc-2" />);
    await act(async () => { lateReceipt.resolve('OLD receipt must not display'); await lateReceipt.promise; });
    expect(screen.queryByText('OLD receipt must not display')).not.toBeInTheDocument();
    view.rerender(<Harness owner="pc-1" />);
    expect(await screen.findByRole('button', { name: 'Retry exact command' })).toBeEnabled();
    await act(async () => { lateRead.resolve(fact('pc-2')); await lateRead.promise; });
    expect(screen.queryByText('pc-2 objective')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Retry exact command' }));
    await waitFor(() => expect(command).toHaveBeenCalledTimes(2));
    expect(command.mock.calls[1]!).toEqual(original);
    expect(current.mock.calls.filter(([owner]) => owner === 'pc-2')).toHaveLength(1);
  });

  it('rejects a late mutation receipt even after switching back to the same canonical owner', async () => {
    const receipt = deferred<string>();
    command.mockReturnValueOnce(receipt.promise);
    const view = render(<Harness />);
    await create();
    view.rerender(<Harness owner="pc-2" />);
    expect(await screen.findByText('No recorded goal.')).toBeInTheDocument();
    view.rerender(<Harness owner="pc-1" />);
    expect(await screen.findByText('Sending command…')).toBeInTheDocument();
    const readCount = current.mock.calls.length;
    await act(async () => { receipt.resolve('stale same-owner receipt'); await receipt.promise; });
    expect(await screen.findByRole('button', { name: 'Retry exact command' })).toBeEnabled();
    expect(screen.queryByText('stale same-owner receipt')).not.toBeInTheDocument();
    expect(current).toHaveBeenCalledTimes(readCount);
  });

  it('fences late control and history reads on owner switchback, not just current-goal reads', async () => {
    const oldControl = deferred<{ version: number; stopped: boolean } | null>();
    const oldHistory = deferred<GoalEntry[]>();
    control.mockReturnValueOnce(oldControl.promise).mockResolvedValue({ version: 2, stopped: false });
    history.mockReturnValueOnce(oldHistory.promise).mockResolvedValue([]);
    const view = render(<Harness />);
    view.rerender(<Harness owner="pc-2" />);
    expect(await screen.findByText('No recorded goal.')).toBeInTheDocument();
    view.rerender(<Harness owner="pc-1" />);
    expect(await screen.findByText('Recorded goal history (0)')).toBeInTheDocument();
    await act(async () => {
      oldControl.resolve({ version: 1, stopped: true }); oldHistory.resolve([fact()]);
      await Promise.all([oldControl.promise, oldHistory.promise]);
    });
    expect(screen.getByText('GOAL none')).toBeInTheDocument();
    expect(screen.getByText(/Control v2 · Stop latch released/)).toBeInTheDocument();
    expect(screen.getByText('Recorded goal history (0)')).toBeInTheDocument();
  });

  it('fences the factual refetch started by a valid receipt across owner switchback', async () => {
    const refetch = deferred<GoalEntry | null>();
    current.mockResolvedValueOnce(null).mockReturnValueOnce(refetch.promise).mockResolvedValue(null);
    const view = render(<Harness />);
    await create();
    await waitFor(() => expect(current).toHaveBeenCalledTimes(2));
    view.rerender(<Harness owner="pc-2" />);
    expect(await screen.findByText('No recorded goal.')).toBeInTheDocument();
    view.rerender(<Harness owner="pc-1" />);
    expect(await screen.findByText('No recorded goal.')).toBeInTheDocument();
    await act(async () => { refetch.resolve(fact()); await refetch.promise; });
    expect(screen.getByText('GOAL none')).toBeInTheDocument();
    expect(screen.queryByText('pc-1 objective')).not.toBeInTheDocument();
  });

  it('does not let an earlier same-owner parallel read overwrite a newer explicit refresh', async () => {
    const oldRead = deferred<GoalEntry | null>();
    current.mockReturnValueOnce(oldRead.promise).mockResolvedValue(fact('pc-1', 2));
    render(<Harness />);
    fireEvent.click(screen.getByRole('button', { name: 'Refresh goals' }));
    expect(await screen.findByText('GOAL Underway · v2')).toBeInTheDocument();
    await act(async () => { oldRead.resolve(fact('pc-1', 1)); await oldRead.promise; });
    expect(screen.getByText('GOAL Underway · v2')).toBeInTheDocument();
    expect(screen.queryByText('GOAL Underway · v1')).not.toBeInTheDocument();
  });

  it('ignores receipts and subsequent refetch after unmount', async () => {
    const receipt = deferred<string>();
    command.mockReturnValueOnce(receipt.promise);
    const view = render(<Harness />);
    await create();
    view.unmount();
    await act(async () => { receipt.resolve('recorded after unmount'); await receipt.promise; });
    expect(current).toHaveBeenCalledTimes(1);
  });

  it('uses independent factual goal/control CAS values and explicit State reason, not execution inference', async () => {
    current.mockResolvedValue(fact('pc-1', 7));
    control.mockResolvedValue({ version: 12, stopped: false });
    history.mockResolvedValue([fact('pc-1', 1)]);
    render(<Harness />);
    fireEvent.click(await screen.findByRole('button', { name: 'Stop goal work' }));
    await waitFor(() => expect(command).toHaveBeenCalledTimes(1));
    expect(command.mock.calls[0]![1]).toEqual({ kind: 'stop', body: { request_id: expect.any(String), command: 'stop', expected_control_version: 12 } });
    await waitFor(() => expect(screen.getByRole('button', { name: 'Record state' })).toBeEnabled());
    fireEvent.change(screen.getByLabelText('Explicit goal state'), { target: { value: 'complete' } });
    expect(screen.getByRole('button', { name: 'Record state' })).toBeDisabled();
    fireEvent.change(screen.getByLabelText('State reason'), { target: { value: '  verified by user  ' } });
    fireEvent.click(screen.getByRole('button', { name: 'Record state' }));
    await waitFor(() => expect(command).toHaveBeenCalledTimes(2));
    expect(command.mock.calls[1]![1]).toEqual({ kind: 'state', body: { request_id: expect.any(String), expected_version: 7, goal_id: 'goal-1', disposition: { state: 'complete', reason: '  verified by user  ' } } });
  });

  it('uses CAS zero only from successful null control read and never auto-releases Stop', async () => {
    render(<Harness />);
    fireEvent.click(await screen.findByRole('button', { name: 'Stop goal work' }));
    await waitFor(() => expect(command).toHaveBeenCalledTimes(1));
    expect(command.mock.calls[0]![1]).toMatchObject({ kind: 'stop', body: { expected_control_version: 0, command: 'stop' } });
    control.mockResolvedValue({ version: 1, stopped: true });
    await waitFor(() => expect(current).toHaveBeenCalledTimes(2));
    fireEvent.click(screen.getByRole('button', { name: 'Refresh goals' }));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Stop goal work' })).toBeDisabled());
    expect(screen.queryByRole('button', { name: /Release/ })).not.toBeInTheDocument();
    expect(command).toHaveBeenCalledTimes(1);
  });

  it('paginates immutable history using the last returned version, separately from current', async () => {
    current.mockResolvedValue(fact('pc-1', 103));
    history.mockImplementation((_owner, after) => Promise.resolve(after === 0
      ? Array.from({ length: 100 }, (_, index) => fact('pc-1', index + 1)) : [fact('pc-1', 101), fact('pc-1', 102)]));
    render(<Harness writable={false} />);
    fireEvent.click(await screen.findByText('Recorded goal history (100)'));
    fireEvent.click(screen.getByRole('button', { name: 'More goal history' }));
    expect(await screen.findByText('Recorded goal history (102)')).toBeInTheDocument();
    expect(history.mock.calls[1]!).toEqual(['pc-1', 100]);
    expect(screen.queryByRole('button', { name: 'More goal history' })).not.toBeInTheDocument();
    expect(screen.getByText('GOAL Underway · v103')).toBeInTheDocument();
  });

  it('does not append stale pagination after owner switchback', async () => {
    const latePage = deferred<GoalEntry[]>();
    history.mockResolvedValueOnce(Array.from({ length: 100 }, (_, index) => fact('pc-1', index + 1)))
      .mockReturnValueOnce(latePage.promise).mockResolvedValue([]);
    const view = render(<Harness writable={false} />);
    fireEvent.click(await screen.findByText('Recorded goal history (100)'));
    fireEvent.click(screen.getByRole('button', { name: 'More goal history' }));
    expect(history).toHaveBeenLastCalledWith('pc-1', 100);
    view.rerender(<Harness owner="pc-2" writable={false} />);
    expect(await screen.findByText('Recorded goal history (0)')).toBeInTheDocument();
    view.rerender(<Harness owner="pc-1" writable={false} />);
    expect(await screen.findByText('Recorded goal history (0)')).toBeInTheDocument();
    await act(async () => { latePage.resolve([fact('pc-1', 101)]); await latePage.promise; });
    expect(screen.getByText('Recorded goal history (0)')).toBeInTheDocument();
    expect(screen.queryByText('pc-1 objective')).not.toBeInTheDocument();
  });

  it('fails closed on a rejected history page until a fresh factual read succeeds', async () => {
    history.mockResolvedValueOnce(Array.from({ length: 100 }, (_, index) => fact('pc-1', index + 1)))
      .mockRejectedValueOnce(new Error('Unordered goal history.')).mockResolvedValue([]);
    render(<Harness />);
    fireEvent.click(await screen.findByText('Recorded goal history (100)'));
    fireEvent.click(screen.getByRole('button', { name: 'More goal history' }));
    expect(await screen.findByText('Unordered goal history.')).toBeInTheDocument();
    expect(screen.queryByLabelText('New objective')).not.toBeInTheDocument();
    expect(command).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Refresh goals' }));
    expect(await screen.findByRole('button', { name: 'Stop goal work' })).toBeEnabled();
    expect(screen.getByText('Recorded goal history (0)')).toBeInTheDocument();
  });

  it('makes no goal request for a nonordinary/null owner', () => {
    render(<Harness owner={null} />);
    expect(current).not.toHaveBeenCalled();
    expect(control).not.toHaveBeenCalled();
    expect(history).not.toHaveBeenCalled();
    expect(command).not.toHaveBeenCalled();
  });
});
