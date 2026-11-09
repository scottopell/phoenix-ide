import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { api, ApiResponseError } from '../api';
import type { Reminder } from '../generated/Reminder';
import { RemindersPanel } from './RemindersPanel';
import { formatReminderTime, normalizeReminderLocalInput, reminderLocalInput } from './reminderTime';

function reminder(overrides: Partial<Reminder> = {}): Reminder {
  return { id: 'rem-1', product_conversation_id: 'product-1', text: 'Inspect Safari',
    due_at_us: Date.parse('2030-01-01T19:30:00Z') * 1000, timezone: 'America/New_York',
    revision: 1, created_at_us: 1, state: { kind: 'scheduled' }, ...overrides };
}

beforeEach(() => {
  vi.spyOn(api, 'listReminders').mockResolvedValue([]);
  vi.spyOn(api, 'manageReminder').mockResolvedValue();
});
afterEach(() => { vi.restoreAllMocks(); });

describe('reminder visibility and management', () => {
  it('uses a compact, accessible status for an unloaded list, not the card panel or an empty claim', async () => {
    vi.mocked(api.listReminders).mockRejectedValueOnce(new Error('Load failed')).mockResolvedValue([]);
    render(<RemindersPanel conversationId="owner" />);
    await screen.findByText('Reminders not loaded');
    const status = document.querySelector('.reminders-announcement');
    expect(status).toHaveTextContent('Reminder list: Reminders not loaded');
    expect(status).toHaveAttribute('aria-live', 'polite');
    expect(screen.queryByRole('region', { name: 'Reminders' })).not.toBeInTheDocument();
    const details = screen.getByText('Details').closest('details');
    expect(details).not.toHaveAttribute('open');
    fireEvent.click(screen.getByText('Details'));
    expect(details).toHaveAttribute('open');
    expect(details).toHaveTextContent('Load failed');
    fireEvent.click(screen.getByRole('button', { name: 'Retry reminders' }));
    await waitFor(() => expect(screen.queryByText('Reminders not loaded')).not.toBeInTheDocument());
    expect(screen.queryByRole('region', { name: 'Reminders' })).not.toBeInTheDocument();
    expect(api.listReminders).toHaveBeenCalledTimes(2);
  });

  it('keeps the live region mounted while the first request is pending and announces its failure', async () => {
    let reject!: (error: Error) => void;
    vi.mocked(api.listReminders).mockImplementationOnce(() => new Promise<Reminder[]>((_resolve, fail) => { reject = fail; }));
    render(<RemindersPanel conversationId="owner" />);
    const status = document.querySelector('.reminders-announcement');
    expect(status).toBeEmptyDOMElement();
    await act(async () => { reject(new Error('Load failed')); });
    expect(status).toHaveTextContent('Reminder list: Reminders not loaded');
  });

  it('retains due cards as explicitly stale through repeated failures, then clears the status on reconnect', async () => {
    vi.mocked(api.listReminders).mockResolvedValueOnce([reminder({ state: { kind: 'due', occurred_at_us: 2 } })])
      .mockRejectedValueOnce(new Error('Load failed')).mockRejectedValueOnce(new Error('Load failed'))
      .mockResolvedValue([reminder({ state: { kind: 'due', occurred_at_us: 2 } })]);
    render(<RemindersPanel conversationId="owner" />);
    expect(await screen.findByText('Reminder due')).toBeInTheDocument();
    fireEvent.focus(window);
    await screen.findByText('Reminders may be out of date');
    const status = document.querySelector('.reminders-announcement');
    expect(status).toHaveTextContent('Reminder list: Reminders may be out of date');
    expect(screen.getByText('Inspect Safari')).toBeInTheDocument();
    expect(document.querySelectorAll('.reminders-announcement')).toHaveLength(1);
    fireEvent.focus(window);
    await waitFor(() => expect(api.listReminders).toHaveBeenCalledTimes(3));
    expect(document.querySelectorAll('.reminders-announcement')).toHaveLength(1);
    window.dispatchEvent(new Event('online'));
    await waitFor(() => expect(screen.queryByText('Reminders may be out of date')).not.toBeInTheDocument());
    expect(screen.getByText('Reminder due')).toBeInTheDocument();
    expect(api.listReminders).toHaveBeenCalledTimes(4);
  });

  it('distinguishes a previously loaded empty list from an unknown initial failure', async () => {
    vi.mocked(api.listReminders).mockResolvedValueOnce([]).mockRejectedValueOnce(new Error('Load failed'));
    render(<RemindersPanel conversationId="owner" />);
    await waitFor(() => expect(api.listReminders).toHaveBeenCalledTimes(1));
    fireEvent.focus(window);
    await screen.findByText('Reminders may be out of date');
    expect(document.querySelector('.reminders-announcement')).toHaveTextContent('Reminders may be out of date');
    expect(screen.queryByRole('region', { name: 'Reminders' })).not.toBeInTheDocument();
  });

  it('keeps an uncertain cancellation actionable without silently replaying the mutation', async () => {
    vi.mocked(api.listReminders).mockResolvedValueOnce([reminder()]).mockRejectedValueOnce(new Error('Load failed'));
    vi.mocked(api.manageReminder).mockRejectedValue(new Error('Network error'));
    render(<RemindersPanel conversationId="owner" />);
    fireEvent.click(await screen.findByRole('button', { name: 'Cancel reminder' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Could not confirm cancel reminder: Network error');
    expect(screen.getByText('Upcoming')).toBeInTheDocument();
    await screen.findByText('Reminders may be out of date');
    expect(document.querySelector('.reminders-announcement')).toHaveTextContent('Reminders may be out of date');
    expect(api.manageReminder).toHaveBeenCalledTimes(1);
  });

  it.each(['cancel', 'reschedule'] as const)('shows saved truth after an unconfirmed %s response and a fresh read', async (operation) => {
    const saved = operation === 'cancel'
      ? reminder({ state: { kind: 'cancelled', cancelled_at_us: 2 } })
      : reminder({ revision: 2, due_at_us: Date.parse('2030-02-01T15:30:00Z') * 1000 });
    vi.mocked(api.listReminders).mockResolvedValueOnce([reminder()])
      .mockResolvedValueOnce([reminder()]).mockResolvedValue([saved]);
    vi.mocked(api.manageReminder).mockRejectedValueOnce(new Error('Network response lost'));
    render(<RemindersPanel conversationId="owner" />);
    if (operation === 'reschedule') {
      fireEvent.click(await screen.findByRole('button', { name: 'Reschedule' }));
      fireEvent.change(screen.getByLabelText('Local date and time'), { target: { value: '2030-02-01T10:30' } });
      fireEvent.click(screen.getByRole('button', { name: 'Save reminder time' }));
    } else {
      fireEvent.click(await screen.findByRole('button', { name: 'Cancel reminder' }));
    }
    await waitFor(() => expect(api.listReminders).toHaveBeenCalledTimes(2));
    expect(screen.getByRole('alert')).toHaveTextContent(`Could not confirm ${operation} reminder`);
    fireEvent.focus(window);
    await waitFor(() => expect(api.listReminders).toHaveBeenCalledTimes(3));
    await waitFor(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument());
    expect(screen.getByText(operation === 'cancel' ? 'Cancelled' : 'Upcoming')).toBeInTheDocument();
    if (operation === 'reschedule') expect(screen.getByText(/Feb 1, 2030/)).toBeInTheDocument();
    expect(screen.queryByText(/Could not (confirm )?(cancel|reschedule) reminder/)).not.toBeInTheDocument();
    expect(api.manageReminder).toHaveBeenCalledTimes(1);
  });

  it('retains uncertainty when a bounded successful read omits the affected reminder', async () => {
    vi.mocked(api.listReminders).mockResolvedValueOnce([reminder()]).mockResolvedValue([]);
    vi.mocked(api.manageReminder).mockRejectedValueOnce(new Error('Network response lost'));
    render(<RemindersPanel conversationId="owner" />);
    fireEvent.click(await screen.findByRole('button', { name: 'Cancel reminder' }));
    await waitFor(() => expect(api.listReminders).toHaveBeenCalledTimes(2));
    expect(screen.getByRole('alert')).toHaveTextContent('Could not confirm cancel reminder');
    expect(api.manageReminder).toHaveBeenCalledTimes(1);
  });

  it('preserves one reminder’s unconfirmed outcome when managing another', async () => {
    const other = reminder({ id: 'rem-2', text: 'Other reminder' });
    vi.mocked(api.listReminders).mockResolvedValueOnce([reminder(), other]).mockRejectedValueOnce(new Error('Load failed'));
    vi.mocked(api.manageReminder).mockRejectedValueOnce(new Error('Network response lost'))
      .mockRejectedValueOnce(new ApiResponseError('Already due', 409));
    render(<RemindersPanel conversationId="owner" />);
    fireEvent.click((await screen.findAllByRole('button', { name: 'Cancel reminder' }))[0]!);
    await waitFor(() => expect(api.listReminders).toHaveBeenCalledTimes(2));
    fireEvent.click(screen.getAllByRole('button', { name: 'Reschedule' })[1]!);
    expect(screen.getByRole('alert')).toHaveTextContent('Could not confirm cancel reminder');
    fireEvent.click(screen.getAllByRole('button', { name: 'Cancel reminder' })[1]!);
    await waitFor(() => expect(screen.getAllByRole('alert')).toHaveLength(2));
    expect(screen.getByText('Could not confirm cancel reminder: Network response lost')).toBeInTheDocument();
    expect(screen.getByText('Could not cancel reminder: Already due')).toBeInTheDocument();
    expect(api.manageReminder).toHaveBeenCalledTimes(2);
  });

  it('keeps a definite rejection for one reminder while editing another', async () => {
    const other = reminder({ id: 'rem-2', text: 'Other reminder' });
    vi.mocked(api.listReminders).mockResolvedValue([reminder(), other]);
    vi.mocked(api.manageReminder).mockRejectedValueOnce(new ApiResponseError('Already due', 409));
    render(<RemindersPanel conversationId="owner" />);
    fireEvent.click((await screen.findAllByRole('button', { name: 'Cancel reminder' }))[0]!);
    expect(await screen.findByRole('alert')).toHaveTextContent('Could not cancel reminder: Already due');
    fireEvent.click(screen.getAllByRole('button', { name: 'Reschedule' })[1]!);
    expect(screen.getByRole('alert')).toHaveTextContent('Could not cancel reminder: Already due');
    expect(api.manageReminder).toHaveBeenCalledTimes(1);
  });

  it('treats a server error after a possible commit as unconfirmed', async () => {
    vi.mocked(api.listReminders).mockResolvedValueOnce([reminder()])
      .mockResolvedValue([reminder({ state: { kind: 'cancelled', cancelled_at_us: 2 } })]);
    vi.mocked(api.manageReminder).mockRejectedValueOnce(new ApiResponseError('Internal error', 500));
    render(<RemindersPanel conversationId="owner" />);
    fireEvent.click(await screen.findByRole('button', { name: 'Cancel reminder' }));
    await waitFor(() => expect(screen.getByText('Cancelled')).toBeInTheDocument());
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(api.manageReminder).toHaveBeenCalledTimes(1);
  });

  it('resolves an unconfirmed reschedule when the saved deadline has already become due', async () => {
    const dueAt = Date.parse('2030-02-01T15:30:00Z') * 1000;
    vi.mocked(api.listReminders).mockResolvedValueOnce([reminder()])
      .mockResolvedValue([reminder({ revision: 2, due_at_us: dueAt,
        state: { kind: 'due', occurred_at_us: dueAt } })]);
    vi.mocked(api.manageReminder).mockRejectedValueOnce(new Error('Network response lost'));
    render(<RemindersPanel conversationId="owner" />);
    fireEvent.click(await screen.findByRole('button', { name: 'Reschedule' }));
    fireEvent.change(screen.getByLabelText('Local date and time'), { target: { value: '2030-02-01T10:30' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save reminder time' }));
    await waitFor(() => expect(screen.getByText('Reminder due')).toBeInTheDocument());
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(api.manageReminder).toHaveBeenCalledTimes(1);
  });

  it('matches a saved reschedule against the normalized offset sent to the server', async () => {
    vi.mocked(api.listReminders).mockResolvedValueOnce([reminder()])
      .mockResolvedValue([reminder({ revision: 2, due_at_us: Date.parse('2030-02-01T15:30:00Z') * 1000 })]);
    vi.mocked(api.manageReminder).mockRejectedValueOnce(new Error('Network response lost'));
    render(<RemindersPanel conversationId="owner" />);
    fireEvent.click(await screen.findByRole('button', { name: 'Reschedule' }));
    fireEvent.change(screen.getByLabelText('Local date and time'), { target: { value: '2030-02-01T10:30' } });
    fireEvent.change(screen.getByLabelText('UTC offset (optional)'), { target: { value: ' -05:00 ' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save reminder time' }));
    await waitFor(() => expect(api.listReminders).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.getByText(/Feb 1, 2030/)).toBeInTheDocument());
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(api.manageReminder).toHaveBeenCalledWith('owner', 'rem-1', 'reschedule', 1,
      { kind: 'at', local_datetime: '2030-02-01T10:30:00', timezone: 'America/New_York', utc_offset: '-05:00' });
  });

  it('retains a definite HTTP rejection after a successful follow-up read', async () => {
    vi.mocked(api.listReminders).mockResolvedValue([reminder()]);
    vi.mocked(api.manageReminder).mockRejectedValueOnce(new ApiResponseError('Invalid local date', 400));
    render(<RemindersPanel conversationId="owner" />);
    fireEvent.click(await screen.findByRole('button', { name: 'Reschedule' }));
    fireEvent.click(screen.getByRole('button', { name: 'Save reminder time' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Could not reschedule reminder: Invalid local date');
    await waitFor(() => expect(api.listReminders).toHaveBeenCalledTimes(2));
    expect(screen.getByRole('alert')).toHaveTextContent('Could not reschedule reminder: Invalid local date');
    expect(api.manageReminder).toHaveBeenCalledTimes(1);
  });

  it('retains a failed reschedule form and marks the list stale if its follow-up refresh fails', async () => {
    vi.mocked(api.listReminders).mockResolvedValueOnce([reminder()]).mockRejectedValueOnce(new Error('Load failed'));
    vi.mocked(api.manageReminder).mockRejectedValue(new ApiResponseError('Invalid local date', 400));
    render(<RemindersPanel conversationId="owner" />);
    fireEvent.click(await screen.findByRole('button', { name: 'Reschedule' }));
    fireEvent.click(screen.getByRole('button', { name: 'Save reminder time' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Could not reschedule reminder: Invalid local date');
    expect(screen.getByRole('button', { name: 'Save reminder time' })).toBeInTheDocument();
    await screen.findByText('Reminders may be out of date');
    expect(document.querySelector('.reminders-announcement')).toHaveTextContent('Reminders may be out of date');
    expect(api.manageReminder).toHaveBeenCalledTimes(1);
  });

  it('does not present a confirmed cancellation as a refreshed list when the GET fails', async () => {
    vi.mocked(api.listReminders).mockResolvedValueOnce([reminder()]).mockRejectedValueOnce(new Error('Load failed'));
    render(<RemindersPanel conversationId="owner" />);
    fireEvent.click(await screen.findByRole('button', { name: 'Cancel reminder' }));
    await screen.findByText('Reminders may be out of date');
    expect(document.querySelector('.reminders-announcement')).toHaveTextContent('Reminders may be out of date');
    expect(screen.getByText('Cancellation saved — list not refreshed')).toBeInTheDocument();
    expect(screen.queryByText('Upcoming')).not.toBeInTheDocument();
    expect(screen.queryByText('Cancelled')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Cancel reminder' })).not.toBeInTheDocument();
    expect(api.manageReminder).toHaveBeenCalledTimes(1);
  });

  it('keeps unrelated reminders manageable and both confirmed changes honest during failed refreshes', async () => {
    vi.mocked(api.listReminders).mockResolvedValueOnce([reminder(), reminder({ id: 'rem-2', text: 'Second task' })])
      .mockRejectedValue(new Error('Load failed'));
    render(<RemindersPanel conversationId="owner" />);
    expect(await screen.findByText('Second task')).toBeInTheDocument();
    fireEvent.click(screen.getAllByRole('button', { name: 'Cancel reminder' })[0]!);
    await screen.findByText('Cancellation saved — list not refreshed');
    expect(screen.getByRole('button', { name: 'Cancel reminder' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Cancel reminder' }));
    await waitFor(() => expect(screen.getAllByText('Cancellation saved — list not refreshed')).toHaveLength(2));
    expect(screen.queryByRole('button', { name: 'Cancel reminder' })).not.toBeInTheDocument();
    expect(api.manageReminder).toHaveBeenCalledTimes(2);
    expect(vi.mocked(api.manageReminder).mock.calls.map((call) => call[1])).toEqual(['rem-1', 'rem-2']);
  });

  it('aborts a stalled pre-mutation GET and starts a new one after a confirmed cancellation', async () => {
    let oldSignal: AbortSignal | undefined;
    vi.mocked(api.listReminders).mockResolvedValueOnce([reminder()])
      .mockImplementationOnce((_id, signal) => {
        oldSignal = signal;
        return new Promise<Reminder[]>(() => {});
      })
      .mockRejectedValueOnce(new Error('Load failed'));
    render(<RemindersPanel conversationId="owner" />);
    await screen.findByRole('button', { name: 'Cancel reminder' });
    fireEvent.focus(window);
    await waitFor(() => expect(api.listReminders).toHaveBeenCalledTimes(2));
    fireEvent.click(screen.getByRole('button', { name: 'Cancel reminder' }));
    await screen.findByText('Cancellation saved — list not refreshed');
    await waitFor(() => expect(api.listReminders).toHaveBeenCalledTimes(3));
    expect(oldSignal?.aborted).toBe(true);
    await screen.findByText('Reminders may be out of date');
    expect(screen.queryByText('Upcoming')).not.toBeInTheDocument();
  });

  it('shows upcoming and durable due cards independently of a model response', async () => {
    vi.mocked(api.listReminders).mockResolvedValue([reminder(), reminder({ id: 'due', text: 'Capture storage', state: { kind: 'due', occurred_at_us: 2 } })]);
    render(<RemindersPanel conversationId="owner" />);
    expect(await screen.findByText('Reminder due')).toBeInTheDocument();
    expect(screen.getByText('Upcoming')).toBeInTheDocument();
    expect(screen.getByText('Capture storage')).toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: 'Cancel reminder' })).toHaveLength(1);
    expect(screen.getAllByText(/America\/New_York/)).toHaveLength(2);
  });

  it('formats the saved IANA zone and sends datetime-local minutes with seconds', () => {
    expect(reminderLocalInput(reminder())).toBe('2030-01-01T14:30:00');
    expect(formatReminderTime(reminder())).toContain('GMT-5');
    expect(normalizeReminderLocalInput('2030-01-01T14:30')).toBe('2030-01-01T14:30:00');
  });

  it('cancels the observed revision then refreshes the durable record', async () => {
    vi.mocked(api.listReminders).mockResolvedValueOnce([reminder()]).mockResolvedValue([reminder({ state: { kind: 'cancelled', cancelled_at_us: 2 } })]);
    render(<RemindersPanel conversationId="owner" />);
    fireEvent.click(await screen.findByRole('button', { name: 'Cancel reminder' }));
    await waitFor(() => expect(api.manageReminder).toHaveBeenCalledWith('owner', 'rem-1', 'cancel', 1, undefined));
    expect(await screen.findByText('Cancelled')).toBeInTheDocument();
  });

  it('keeps the edit revision when a background refresh changes the saved reminder', async () => {
    vi.mocked(api.listReminders).mockResolvedValueOnce([reminder()]).mockResolvedValue([reminder({ revision: 2 })]);
    render(<RemindersPanel conversationId="owner" />);
    fireEvent.click(await screen.findByRole('button', { name: 'Reschedule' }));
    fireEvent.focus(window);
    await waitFor(() => expect(api.listReminders).toHaveBeenCalledTimes(2));
    fireEvent.change(screen.getByLabelText('Local date and time'), { target: { value: '2030-02-01T10:30' } });
    fireEvent.change(screen.getByLabelText('UTC offset (optional)'), { target: { value: '-05:00' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save reminder time' }));
    await waitFor(() => expect(api.manageReminder).toHaveBeenCalledWith('owner', 'rem-1', 'reschedule', 1, {
      kind: 'at', local_datetime: '2030-02-01T10:30:00', timezone: 'America/New_York', utc_offset: '-05:00',
    }));
  });

  it('reloads on revision conflict and requires a fresh edit', async () => {
    vi.mocked(api.listReminders).mockResolvedValue([reminder()]);
    vi.mocked(api.manageReminder).mockRejectedValue(new ApiResponseError('This reminder changed.', 409));
    render(<RemindersPanel conversationId="owner" />);
    fireEvent.click(await screen.findByRole('button', { name: 'Reschedule' }));
    fireEvent.click(screen.getByRole('button', { name: 'Save reminder time' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('This reminder changed.');
    expect(screen.queryByRole('button', { name: 'Save reminder time' })).not.toBeInTheDocument();
    await waitFor(() => expect(api.listReminders).toHaveBeenCalledTimes(2));
  });

  it('ignores a previous route response and aborts its request', async () => {
    let resolveOld!: (rows: Reminder[]) => void;
    vi.mocked(api.listReminders).mockImplementation((id) => id === 'old' ? new Promise((resolve) => { resolveOld = resolve; }) : Promise.resolve([reminder({ text: 'New owner reminder' })]));
    const { rerender } = render(<RemindersPanel conversationId="old" />);
    const oldSignal = vi.mocked(api.listReminders).mock.calls[0]?.[1];
    rerender(<RemindersPanel conversationId="new" />);
    expect(await screen.findByText('New owner reminder')).toBeInTheDocument();
    await act(async () => { resolveOld([reminder({ text: 'Stale owner reminder' })]); });
    expect(oldSignal?.aborted).toBe(true);
    expect(screen.queryByText('Stale owner reminder')).not.toBeInTheDocument();
  });

  it('coalesces refresh requests while a fetch is outstanding', async () => {
    let resolve!: (rows: Reminder[]) => void;
    vi.mocked(api.listReminders).mockImplementationOnce(() => new Promise((done) => { resolve = done; })).mockResolvedValue([]);
    render(<RemindersPanel conversationId="owner" />);
    fireEvent.focus(window);
    fireEvent.focus(window);
    expect(api.listReminders).toHaveBeenCalledTimes(1);
    await act(async () => { resolve([]); });
    expect(api.listReminders).toHaveBeenCalledTimes(2);
  });
});
