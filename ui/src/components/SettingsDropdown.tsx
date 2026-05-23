import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { api, type CodexLoginPreflight, type NotificationSettings } from '../api';
import { refreshModels } from '../modelsPoller';
import {
  getBrowserNotificationPermission,
  getNotificationRuntimeSettings,
  loadNotificationSettings,
  updateNotificationRuntimeSettings,
} from '../notifications';

type BrowserPermission = NotificationPermission | 'unsupported';

interface Props {
  theme: 'dark' | 'light';
  onToggleTheme: () => void;
  codexPreflight: CodexLoginPreflight | null;
  onPreflightInvalidated: () => void;
  /** Render inside the icon-strip of the collapsed sidebar. */
  compact?: boolean;
}

// Hand-drawn gear — wobbly path with slight imperfections in the spokes and
// teeth so it doesn't read as a generic lucide stroke icon.
const GearIcon = () => (
  <svg
    width="18"
    height="18"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    strokeWidth="1.6"
    strokeLinecap="round"
    strokeLinejoin="round"
    aria-hidden="true"
  >
    {/* outer rim with sketchy teeth — not perfectly symmetric */}
    <path d="M12.2 2.4 L13.6 4.7 L16.2 4.1 L16.8 6.7 L19.3 7.6 L18.5 10.1 L20.5 11.9 L18.6 13.8 L19.4 16.4 L16.9 17.2 L16.3 19.8 L13.7 19.2 L11.9 21.5 L10.2 19.3 L7.6 19.9 L7 17.3 L4.5 16.4 L5.3 13.9 L3.3 12 L5.2 10.2 L4.4 7.7 L7 6.8 L7.7 4.2 L10.3 4.8 Z" />
    {/* center pivot — slight oval, not a perfect circle */}
    <path d="M14.5 12 a2.6 2.4 0 1 1 -5.2 0 a2.6 2.4 0 1 1 5.2 0" />
  </svg>
);

function shortAccount(id: string | null): string {
  if (!id) return 'unknown';
  if (id.length <= 12) return id;
  return `${id.slice(0, 4)}…${id.slice(-4)}`;
}

export function SettingsDropdown({
  theme,
  onToggleTheme,
  codexPreflight,
  onPreflightInvalidated,
  compact = false,
}: Props) {
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const menuRef = useRef<HTMLDivElement | null>(null);
  const [menuPos, setMenuPos] = useState<{ top: number; left: number } | null>(null);

  useEffect(() => {
    if (!open) return undefined;
    const onDocClick = (e: MouseEvent) => {
      const target = e.target as Node;
      if (wrapRef.current?.contains(target)) return;
      if (menuRef.current?.contains(target)) return;
      setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false);
    };
    document.addEventListener('mousedown', onDocClick);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDocClick);
      document.removeEventListener('keydown', onKey);
    };
  }, [open]);

  // Position the menu in viewport coords so it escapes the sidebar's
  // overflow:hidden clip. Prefer right-align under the trigger; clamp into
  // the viewport so the left edge can't go off-screen on a narrow sidebar.
  useLayoutEffect(() => {
    if (!open) { setMenuPos(null); return; }
    const trigger = triggerRef.current;
    const menu = menuRef.current;
    if (!trigger || !menu) return;
    const tRect = trigger.getBoundingClientRect();
    const menuWidth = menu.offsetWidth;
    const margin = 8;
    let left = tRect.right - menuWidth;
    if (left < margin) left = margin;
    if (left + menuWidth > window.innerWidth - margin) {
      left = window.innerWidth - menuWidth - margin;
    }
    const top = tRect.bottom + 6;
    setMenuPos({ top, left });
  }, [open]);

  return (
    <div className={`settings-dropdown-wrap${compact ? ' settings-dropdown-wrap--compact' : ''}`} ref={wrapRef}>
      <button
        ref={triggerRef}
        type="button"
        className="settings-dropdown-trigger"
        title="Settings"
        aria-label="Settings"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
      >
        <GearIcon />
      </button>
      {open && (
        <div
          ref={menuRef}
          className="settings-dropdown-menu"
          role="menu"
          style={menuPos ? { top: `${menuPos.top}px`, left: `${menuPos.left}px` } : { visibility: 'hidden' }}
        >
          <ThemeSection theme={theme} onToggle={onToggleTheme} />
          {codexPreflight?.already_signed_in && (
            <CodexSection
              preflight={codexPreflight}
              onPreflightInvalidated={onPreflightInvalidated}
              onCloseMenu={() => setOpen(false)}
            />
          )}
          <NotificationsSection />
          <VersionFooter />
        </div>
      )}
    </div>
  );
}

function ThemeSection({ theme, onToggle }: { theme: 'dark' | 'light'; onToggle: () => void }) {
  return (
    <section className="settings-section">
      <h3 className="settings-section__title">Theme</h3>
      <div className="settings-theme-row">
        <button
          type="button"
          className={`settings-theme-btn${theme === 'light' ? ' active' : ''}`}
          onClick={() => { if (theme !== 'light') onToggle(); }}
        >
          Light
        </button>
        <button
          type="button"
          className={`settings-theme-btn${theme === 'dark' ? ' active' : ''}`}
          onClick={() => { if (theme !== 'dark') onToggle(); }}
        >
          Dark
        </button>
      </div>
    </section>
  );
}

function CodexSection({
  preflight,
  onPreflightInvalidated,
  onCloseMenu,
}: {
  preflight: CodexLoginPreflight;
  onPreflightInvalidated: () => void;
  onCloseMenu: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const identity = preflight.account_email ?? (preflight.account_id ? shortAccount(preflight.account_id) : null);

  const handleSignOut = useCallback(async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await api.codexSignout();
      await refreshModels();
      onPreflightInvalidated();
      onCloseMenu();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }, [busy, onPreflightInvalidated, onCloseMenu]);

  return (
    <section className="settings-section">
      <h3 className="settings-section__title">LLM Provider</h3>
      <div className="settings-codex-row">
        <div className="settings-codex-identity">
          <div className="settings-codex-provider">Codex</div>
          {identity && (
            <div className="settings-codex-account" title={preflight.account_id ?? undefined}>
              {preflight.account_email ? identity : <code>{identity}</code>}
            </div>
          )}
        </div>
        <button
          type="button"
          className="settings-signout-btn"
          onClick={() => { void handleSignOut(); }}
          disabled={busy}
        >
          {busy ? 'Signing out…' : 'Sign out'}
        </button>
      </div>
      {error && <div className="settings-section__error">{error}</div>}
    </section>
  );
}

function NotificationsSection() {
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
    <section className="settings-section">
      <h3 className="settings-section__title">Desktop notifications</h3>
      <label className="settings-checkbox">
        <input
          type="checkbox"
          checked={settings.enabled}
          onChange={(e) => setFlag('enabled', e.target.checked)}
        />
        Enable browser notifications
      </label>
      <div className="settings-section__hint">Browser permission: {permission}</div>
      {permission === 'default' && (
        <button type="button" className="settings-inline-btn" onClick={requestPermission}>
          Grant browser permission
        </button>
      )}
      {permission === 'denied' && (
        <div className="settings-section__hint">Denied — re-enable in browser settings.</div>
      )}
      {permission === 'unsupported' && (
        <div className="settings-section__hint">Browser does not support notifications.</div>
      )}
      <div className="settings-checkbox-group">
        <label className="settings-checkbox">
          <input type="checkbox" checked={settings.notify_task_approval} onChange={(e) => setFlag('notify_task_approval', e.target.checked)} />
          Task approval
        </label>
        <label className="settings-checkbox">
          <input type="checkbox" checked={settings.notify_question} onChange={(e) => setFlag('notify_question', e.target.checked)} />
          Questions
        </label>
        <label className="settings-checkbox">
          <input type="checkbox" checked={settings.notify_error} onChange={(e) => setFlag('notify_error', e.target.checked)} />
          Errors and context full
        </label>
        <label className="settings-checkbox">
          <input type="checkbox" checked={settings.notify_idle} onChange={(e) => setFlag('notify_idle', e.target.checked)} />
          Long task finished
        </label>
      </div>
      {saving && <div className="settings-section__hint">Saving…</div>}
      {error && <div className="settings-section__error">{error}</div>}
    </section>
  );
}

function VersionFooter() {
  const [info, setInfo] = useState<{ version: string; git_sha: string } | null>(null);
  useEffect(() => {
    let cancelled = false;
    api.getVersion()
      .then((v) => { if (!cancelled) setInfo(v); })
      .catch(() => { /* footer just hides on failure */ });
    return () => { cancelled = true; };
  }, []);
  if (!info) return null;
  return (
    <div className="settings-version-footer">
      <span>v{info.version}</span>
      <code title="Git SHA">{info.git_sha}</code>
    </div>
  );
}
