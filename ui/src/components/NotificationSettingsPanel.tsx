import { useState, useEffect, useCallback } from 'react';
import { api, type NotificationSettings } from '../api';
import {
  DEFAULT_NOTIFICATION_SETTINGS,
  getBrowserNotificationPermission,
  updateNotificationRuntimeSettings,
} from '../notifications';

type BrowserPermission = NotificationPermission | 'unsupported';

interface Props {
  compact?: boolean;
}

export function NotificationSettingsPanel({ compact = false }: Props) {
  const [settings, setSettings] = useState<NotificationSettings>(DEFAULT_NOTIFICATION_SETTINGS);
  const [permission, setPermission] = useState<BrowserPermission>(() => getBrowserNotificationPermission());
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    api.getNotificationSettings()
      .then((loaded) => {
        if (cancelled) return;
        setSettings(loaded);
        updateNotificationRuntimeSettings(loaded);
      })
      .catch((err) => {
        if (!cancelled) setError(err instanceof Error ? err.message : 'Failed to load notification settings');
      });
    return () => { cancelled = true; };
  }, []);

  const save = useCallback(async (next: NotificationSettings) => {
    setSettings(next);
    updateNotificationRuntimeSettings(next);
    setSaving(true);
    setError(null);
    try {
      const saved = await api.updateNotificationSettings(next);
      setSettings(saved);
      updateNotificationRuntimeSettings(saved);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to save notification settings');
    } finally {
      setSaving(false);
    }
  }, []);

  const setFlag = useCallback((key: keyof NotificationSettings, value: boolean) => {
    void save({ ...settings, [key]: value });
  }, [save, settings]);

  const requestPermission = useCallback(async () => {
    if (!('Notification' in window) || Notification.permission !== 'default') {
      setPermission(getBrowserNotificationPermission());
      return;
    }
    const result = await Notification.requestPermission();
    setPermission(result);
  }, []);

  return (
    <div className={compact ? 'notification-settings notification-settings--compact' : 'notification-settings'}>
      <div className="notification-settings__title">Desktop notifications</div>
      <label>
        <input
          type="checkbox"
          checked={settings.enabled}
          onChange={(e) => setFlag('enabled', e.target.checked)}
        />
        Enable browser notifications
      </label>
      <div className="notification-settings__permission">Browser permission: {permission}</div>
      {permission === 'default' && (
        <button type="button" onClick={requestPermission}>Enable desktop notifications</button>
      )}
      {permission === 'denied' && (
        <div className="notification-settings__hint">Permission is denied. Re-enable notifications in your browser settings.</div>
      )}
      {permission === 'unsupported' && (
        <div className="notification-settings__hint">This browser does not support desktop notifications.</div>
      )}
      <div className="notification-settings__events">
        <label>
          <input type="checkbox" checked={settings.notify_task_approval} onChange={(e) => setFlag('notify_task_approval', e.target.checked)} />
          Task approval
        </label>
        <label>
          <input type="checkbox" checked={settings.notify_question} onChange={(e) => setFlag('notify_question', e.target.checked)} />
          Questions
        </label>
        <label>
          <input type="checkbox" checked={settings.notify_error} onChange={(e) => setFlag('notify_error', e.target.checked)} />
          Errors and context full
        </label>
        <label>
          <input type="checkbox" checked={settings.notify_idle} onChange={(e) => setFlag('notify_idle', e.target.checked)} />
          Long task finished
        </label>
      </div>
      {saving && <div className="notification-settings__hint">Saving…</div>}
      {error && <div className="notification-settings__error">{error}</div>}
    </div>
  );
}
