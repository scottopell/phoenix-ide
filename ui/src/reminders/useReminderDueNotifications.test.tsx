import { fireEvent, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { api } from '../api';
import { notifyReminderFeed } from '../notifications';
import { useReminderDueNotifications } from './useReminderDueNotifications';

vi.mock('../notifications', () => ({
  loadNotificationSettings: vi.fn().mockResolvedValue({}),
  notifyReminderFeed: vi.fn(),
}));
afterEach(() => vi.restoreAllMocks());

it('starts every global scan without an old deadline cursor so late-observed reminders remain visible', async () => {
  const first = { reminder: { id: 'new-deadline', product_conversation_id: 'pc', text: 'new', due_at_us: 200, timezone: 'UTC', revision: 1, created_at_us: 1, state: { kind: 'due' as const, occurred_at_us: 300 } }, target: { id: 'c', slug: 'c', title: null, kind: 'ordinary' as const } };
  const late = { ...first, reminder: { ...first.reminder, id: 'late', due_at_us: 100, state: { kind: 'due' as const, occurred_at_us: 400 } } };
  const fetch = vi.spyOn(api, 'reminderDueFeed')
    .mockResolvedValueOnce({ items: [first], next: { due_at_us: 200, reminder_id: 'new-deadline' } })
    .mockResolvedValueOnce({ items: [], next: null })
    .mockResolvedValue({ items: [late, first], next: null });
  renderHook(() => useReminderDueNotifications());
  await waitFor(() => expect(notifyReminderFeed).toHaveBeenCalledWith([first]));
  expect(fetch.mock.calls[1]?.[0]).toEqual({ due_at_us: 200, reminder_id: 'new-deadline' });
  fireEvent.online(window);
  await waitFor(() => expect(notifyReminderFeed).toHaveBeenLastCalledWith([late, first]));
  expect(fetch.mock.calls[2]?.[0]).toBeNull();
});
