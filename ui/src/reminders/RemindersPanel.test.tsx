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
