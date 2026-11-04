import { useCallback, useState } from 'react';
import { api, ApiResponseError } from '../api';
import type { Reminder } from '../generated/Reminder';
import { useReminderRefresh } from './useReminderRefresh';
import './RemindersPanel.css';
import { formatReminderTime, normalizeReminderLocalInput, reminderLocalInput } from './reminderTime';

const STATE_ORDER = { due: 0, scheduled: 1, cancelled: 2 };

type Edit = { reminder: Reminder; local: string; timezone: string; offset: string };

export function RemindersPanel({ conversationId, readOnly = false }: { conversationId: string; readOnly?: boolean }) {
  return <OwnedRemindersPanel key={conversationId} conversationId={conversationId} readOnly={readOnly} />;
}

function OwnedRemindersPanel({ conversationId, readOnly }: { conversationId: string; readOnly: boolean }) {
  const [reminders, setReminders] = useState<Reminder[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [edit, setEdit] = useState<Edit | null>(null);
  const [saving, setSaving] = useState(false);
  const load = useCallback((signal: AbortSignal) => api.listReminders(conversationId, signal), [conversationId]);
  const receive = useCallback((rows: Reminder[]) => { setReminders(rows); setLoadError(null); }, []);
  const fail = useCallback((failure: unknown) => { setLoadError(failure instanceof Error ? failure.message : 'Failed to load reminders'); }, []);
  const refresh = useReminderRefresh(load, receive, fail);

  const manage = async (reminder: Reminder, operation: 'cancel' | 'reschedule') => {
    setSaving(true);
    setError(null);
    try {
      await api.manageReminder(conversationId, reminder.id, operation, reminder.revision,
        operation === 'reschedule' && edit ? {
          kind: 'at', local_datetime: normalizeReminderLocalInput(edit.local), timezone: edit.timezone,
          utc_offset: edit.offset.trim() || null,
        } : undefined);
      setEdit(null);
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : 'Failed to update reminder');
      if (failure instanceof ApiResponseError && failure.status === 409) setEdit(null);
    } finally {
      setSaving(false);
      refresh();
    }
  };

  const displayedReminders = reminders.toSorted((a, b) => STATE_ORDER[a.state.kind] - STATE_ORDER[b.state.kind]);
  if (!reminders.length && !loadError && !error && !edit) return null;
  return <section className="reminders-panel" aria-label="Reminders">
    <h2>Reminders</h2>
    {loadError && <p role="status">{loadError} <button type="button" onClick={refresh}>Retry</button></p>}
    {error && <p role="alert">{error}</p>}
    <ul>{displayedReminders.map((reminder) => <li key={reminder.id} className={`reminder-card reminder-card--${reminder.state.kind}`}>
      <span className="reminder-card__state">{reminder.state.kind === 'due' ? 'Reminder due' : reminder.state.kind === 'cancelled' ? 'Cancelled' : 'Upcoming'}</span>
      <p>{reminder.text}</p>
      <time dateTime={new Date(reminder.due_at_us / 1000).toISOString()}>{formatReminderTime(reminder)}</time>
      {!readOnly && reminder.state.kind === 'scheduled' && <div className="reminder-card__actions">
        <button type="button" disabled={saving} onClick={() => {
          setError(null);
          setEdit({ reminder, local: reminderLocalInput(reminder), timezone: reminder.timezone, offset: '' });
        }}>Reschedule</button>
        <button type="button" disabled={saving} onClick={() => { void manage(reminder, 'cancel'); }}>Cancel reminder</button>
      </div>}
      {edit && edit.reminder.id === reminder.id && reminder.state.kind === 'scheduled' && !readOnly && <form className="reminder-edit" onSubmit={(event) => { event.preventDefault(); void manage(edit.reminder, 'reschedule'); }}>
      <label>Local date and time<input required type="datetime-local" step="1" value={edit.local} onChange={(event) => setEdit({ ...edit, local: event.target.value })} /></label>
      <label>IANA timezone<input required value={edit.timezone} onChange={(event) => setEdit({ ...edit, timezone: event.target.value })} /></label>
      <label>UTC offset (optional)<input placeholder="e.g. -04:00" value={edit.offset} onChange={(event) => setEdit({ ...edit, offset: event.target.value })} /></label>
      <span>For a time that occurs twice, choose the UTC offset shown in the validation message.</span>
      <div className="reminder-card__actions"><button type="submit" disabled={saving}>Save reminder time</button><button type="button" disabled={saving} onClick={() => setEdit(null)}>Dismiss</button></div>
    </form>}
    </li>)}</ul>

  </section>;
}
