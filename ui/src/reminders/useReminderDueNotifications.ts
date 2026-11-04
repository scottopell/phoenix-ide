import { api } from '../api';
import type { ReminderFeedCursor } from '../generated/ReminderFeedCursor';
import type { ReminderNotification } from '../generated/ReminderNotification';
import { loadNotificationSettings, notifyReminderFeed } from '../notifications';
import { useReminderRefresh } from './useReminderRefresh';

async function loadDueFeed(signal: AbortSignal): Promise<ReminderNotification[]> {
  await loadNotificationSettings();
  const items: ReminderNotification[] = [];
  let cursor: ReminderFeedCursor | null = null;
  // Optional connected-page alerts have a bounded catch-up window and scan budget.
  for (let page = 0; page < 10; page += 1) {
    const response = await api.reminderDueFeed(cursor, signal);
    items.push(...response.items);
    cursor = response.next;
    if (!cursor) break;
  }
  return items;
}

const ignoreFeedFailure = () => undefined;

export function useReminderDueNotifications(): void {
  useReminderRefresh(loadDueFeed, notifyReminderFeed, ignoreFeedFailure);
}
