import type { Reminder } from '../generated/Reminder';

export function formatReminderTime(reminder: Reminder): string {
  return `${new Intl.DateTimeFormat(undefined, {
    timeZone: reminder.timezone, year: 'numeric', month: 'short', day: 'numeric',
    hour: 'numeric', minute: '2-digit', second: '2-digit', timeZoneName: 'shortOffset',
  }).format(new Date(reminder.due_at_us / 1000))} (${reminder.timezone})`;
}

export function reminderLocalInput(reminder: Reminder): string {
  const parts = new Intl.DateTimeFormat('en-CA', {
    timeZone: reminder.timezone, year: 'numeric', month: '2-digit', day: '2-digit',
    hour: '2-digit', minute: '2-digit', second: '2-digit', hourCycle: 'h23',
  }).formatToParts(new Date(reminder.due_at_us / 1000));
  const part = (type: Intl.DateTimeFormatPartTypes) => parts.find((item) => item.type === type)?.value ?? '';
  return `${part('year')}-${part('month')}-${part('day')}T${part('hour')}:${part('minute')}:${part('second')}`;
}

export function normalizeReminderLocalInput(value: string): string {
  return value.length === 16 ? `${value}:00` : value;
}
