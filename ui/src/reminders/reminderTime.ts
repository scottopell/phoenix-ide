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

export function reminderLocalTimeHasUniqueInstant(reminder: Reminder): boolean {
  const instant = reminder.due_at_us / 1000;
  const local = reminderLocalInput(reminder);
  const formatter = new Intl.DateTimeFormat('en-US', { timeZone: reminder.timezone, timeZoneName: 'longOffset' });
  const offsets = [instant - 24 * 60 * 60 * 1000, instant, instant + 24 * 60 * 60 * 1000]
    .map((time) => formatter.formatToParts(new Date(time)).find((part) => part.type === 'timeZoneName')?.value);
  for (const [index, value] of offsets.entries()) {
    const offset = value === 'GMT' ? '+00:00' : value?.replace(/^GMT/, '');
    if (!offset || !/^[+-]\d{2}:\d{2}$/.test(offset)) return false;
    const candidate = Date.parse(`${local}${offset}`) * 1000;
    if (!Number.isFinite(candidate) || (index === 1 && candidate !== reminder.due_at_us)) return false;
    if (candidate !== reminder.due_at_us
      && reminderLocalInput({ ...reminder, due_at_us: candidate }) === local) return false;
  }
  return true;
}
