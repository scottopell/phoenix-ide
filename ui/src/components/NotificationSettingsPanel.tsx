import { useState, useEffect, useCallback, useRef } from 'react';
import { api, type NotificationSettings } from '../api';
import {
  getBrowserNotificationPermission,
  getNotificationRuntimeSettings,
  loadNotificationSettings,
  updateNotificationRuntimeSettings,
} from '../notifications';

type BrowserPermission = NotificationPermission | 'unsupported';

interface Props {
  compact?: boolean;
}

export function NotificationSettingsPanel({ compact = false }: Props) {
  const [settings, setSettings] = useState<NotificationSettings>(() => getNotificationRuntimeSettings());
  const [permission, setPermission] = useState<BrowserPermission>(() => getBrowserNotificationPermission());
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const latestSaveRef = useRef(0);
  const saveChainRef = useRef(Promise.resolve());
  const editedRef = useRef(false);

  useEffect(() => {
    let cancelled = false;
    loadNotificationSettings()
      .then((loaded) => {
        if (cancelled || editedRef.current) return;
        setSettings(loaded);
      })
      .catch((err) => {
        if (!cancelled) setError(err instanceof Error ? err.message : 'Failed to load notification settings');
      });
    return () => { cancelled = true; };
  }, []);

  const save = useCallback((next: NotificationSettings) => {
    const saveId = latestSaveRef.current + 1;
    latestSaveRef.current = saveId;
    editedRef.current = true;
    setSettings(next);
    updateNotificationRuntimeSettings(next);
    setSaving(true);
    setError(null);
    saveChainRef.current = saveChainRef.current
      .catch(() => {})
      .then(async () => {
        const saved = await api.updateNotificationSettings(next);
        if (latestSaveRef.current !== saveId) return;
        setSettings(saved);
        updateNotificationRuntimeSettings(saved);
        setSaving(false);
      })
      .catch((err: unknown) => {
        if (latestSaveRef.current === saveId) {
          setError(err instanceof Error ? err.message : 'Failed to save notification settings');
          setSaving(false);
        }
      });
  }, []);

  const setFlag = useCallback((key: keyof NotificationSettings, value: boolean) => {
    setSettings((prev) => {
      const next = { ...prev, [key]: value };
      void save(next);
      return next;
    });
  }, [save]);

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
