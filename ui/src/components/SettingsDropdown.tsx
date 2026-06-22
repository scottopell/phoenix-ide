import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { api, type CodexLoginPreflight, type LlmLanguageSetting, type NotificationSettings } from '../api';
import { refreshModels } from '../modelsPoller';
import { clearCodexQuota, useCodexQuota } from '../codexQuota';
import { CodexQuotaBlock } from './CodexQuotaBlock';
import {
  getBrowserNotificationPermission,
  useNotificationSettings,
} from '../notifications';
import { useDensity } from '../hooks/useDensity';

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
  const titleId = useId();
  const navigate = useNavigate();
  const [menuPos, setMenuPos] = useState<{ top: number; left: number; maxHeight: number } | null>(null);

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
  // overflow:hidden clip. Recompute on resize/scroll/sidebar-animate so
  // the menu tracks the trigger instead of drifting once open. State
  // updates are deduped (only when top/left/maxHeight actually change)
  // and scroll bursts are coalesced via rAF so a fast scroll doesn't
  // queue one React render per scroll event.
  const computePosition = useCallback(() => {
    const trigger = triggerRef.current;
    const menu = menuRef.current;
    if (!trigger || !menu) return;
    const tRect = trigger.getBoundingClientRect();
    const margin = 8;
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    const menuWidth = Math.min(menu.offsetWidth, vw - 2 * margin);
    // Prefer right-aligned to trigger; clamp both edges into viewport, and
    // floor at `margin` last so an oversized menu never produces negative left.
    let left = tRect.right - menuWidth;
    if (left + menuWidth > vw - margin) left = vw - menuWidth - margin;
    if (left < margin) left = margin;
    const top = tRect.bottom + 6;
    const maxHeight = Math.max(120, vh - top - margin);
    setMenuPos((prev) =>
      prev && prev.top === top && prev.left === left && prev.maxHeight === maxHeight
        ? prev
        : { top, left, maxHeight }
    );
  }, []);

  useLayoutEffect(() => {
    if (!open) { setMenuPos(null); return undefined; }
    computePosition();
    let rafId = 0;
    const schedule = () => {
      if (rafId) return;
      rafId = window.requestAnimationFrame(() => {
        rafId = 0;
        computePosition();
      });
    };
    window.addEventListener('resize', schedule);
    window.addEventListener('scroll', schedule, true);
    const ro = typeof ResizeObserver !== 'undefined' ? new ResizeObserver(schedule) : null;
    if (ro && triggerRef.current) ro.observe(triggerRef.current);
    return () => {
      window.removeEventListener('resize', schedule);
      window.removeEventListener('scroll', schedule, true);
      ro?.disconnect();
      if (rafId) window.cancelAnimationFrame(rafId);
    };
  }, [open, computePosition]);

  return (
    <div className={`settings-dropdown-wrap${compact ? ' settings-dropdown-wrap--compact' : ''}`} ref={wrapRef}>
      <button
        ref={triggerRef}
        type="button"
        className="settings-dropdown-trigger"
        title="Settings"
        aria-label="Settings"
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
      >
        <GearIcon />
      </button>
      {open && (
        <div
          ref={menuRef}
          className="settings-dropdown-menu"
          role="dialog"
          aria-labelledby={titleId}
          style={menuPos
            ? { top: `${menuPos.top}px`, left: `${menuPos.left}px`, maxHeight: `${menuPos.maxHeight}px` }
            : { visibility: 'hidden' }}
        >
          <h2 id={titleId} className="settings-dropdown-title">Settings</h2>
          <ThemeSection theme={theme} onToggle={onToggleTheme} />
          <DensitySection />
          {codexPreflight?.already_signed_in && (
            <CodexSection
              preflight={codexPreflight}
              onPreflightInvalidated={onPreflightInvalidated}
              onCloseMenu={() => setOpen(false)}
            />
          )}
          <NotificationsSection />
          <LlmLanguageSection onCloseMenu={() => setOpen(false)} />
          <section className="settings-section">
            <button
              type="button"
              className="settings-inline-btn settings-about-link"
              onClick={() => { setOpen(false); navigate('/usage'); }}
              title="Token usage and cost"
            >
              Usage →
            </button>
            <button
              type="button"
              className="settings-inline-btn settings-about-link"
              onClick={() => { setOpen(false); navigate('/about'); }}
              title="Open deployment details"
            >
              About this deployment →
            </button>
          </section>
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
          title={theme === 'light' ? 'Light theme is active' : 'Switch to light theme'}
        >
          Light
        </button>
        <button
          type="button"
          className={`settings-theme-btn${theme === 'dark' ? ' active' : ''}`}
          onClick={() => { if (theme !== 'dark') onToggle(); }}
          title={theme === 'dark' ? 'Dark theme is active' : 'Switch to dark theme'}
        >
          Dark
        </button>
      </div>
    </section>
  );
}

/**
 * Conversation view density. `Full` paints every message exactly as it always
 * has; `Compact` collapses each agent turn's tool calls into an inline pill
 * strip and short assistant prose into expandable one-liners. Purely
 * presentational, persisted via the density context (localStorage).
 */
function DensitySection() {
  const { density, setDensity } = useDensity();
  return (
    <section className="settings-section">
      <h3 className="settings-section__title">Conversation density</h3>
      <div className="settings-section__hint">
        Compact collapses each turn's tools into a pill strip and folds short
        replies — click to expand. Nothing is hidden.
      </div>
      <div className="settings-theme-row" role="radiogroup" aria-label="Conversation density">
        <button
          type="button"
          role="radio"
          aria-checked={density === 'full'}
          className={`settings-theme-btn${density === 'full' ? ' active' : ''}`}
          onClick={() => setDensity('full')}
          title={density === 'full' ? 'Full density is active' : 'Switch to full density'}
        >
          Full
        </button>
        <button
          type="button"
          role="radio"
          aria-checked={density === 'compact'}
          className={`settings-theme-btn${density === 'compact' ? ' active' : ''}`}
          onClick={() => setDensity('compact')}
          title={density === 'compact' ? 'Compact density is active' : 'Switch to compact density'}
        >
          Compact
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
  const mountedRef = useRef(true);
  const quota = useCodexQuota();
  useEffect(() => {
    mountedRef.current = true;
    return () => { mountedRef.current = false; };
  }, []);

  const identity = preflight.account_email ?? (preflight.account_id ? shortAccount(preflight.account_id) : null);

  const handleSignOut = useCallback(async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await api.codexSignout();
      clearCodexQuota();
      await refreshModels();
      onPreflightInvalidated();
      onCloseMenu();
    } catch (e) {
      if (mountedRef.current) setError(e instanceof Error ? e.message : String(e));
    } finally {
      if (mountedRef.current) setBusy(false);
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
          title={busy ? 'Signing out of Codex…' : 'Sign out of Codex'}
        >
          {busy ? 'Signing out…' : 'Sign out'}
        </button>
      </div>
      {error && <div className="settings-section__error">{error}</div>}
      {quota && <CodexQuotaBlock quota={quota} />}
    </section>
  );
}

function NotificationsSection() {
  // Settings, save ordering, and the saving/error surface all live in the
  // notification policy reducer; this component only renders them and reads
  // browser-owned permission state directly.
  const { settings, saving, error, save } = useNotificationSettings();
  const [permission, setPermission] = useState<BrowserPermission>(() => getBrowserNotificationPermission());
  const mountedRef = useRef(true);

  useEffect(() => {
    mountedRef.current = true;
    return () => { mountedRef.current = false; };
  }, []);

  const setFlag = useCallback((key: keyof NotificationSettings, value: boolean) => {
    save({ ...settings, [key]: value });
  }, [save, settings]);

  const requestPermission = useCallback(async () => {
    if (!('Notification' in window) || Notification.permission !== 'default') {
      if (mountedRef.current) setPermission(getBrowserNotificationPermission());
      return;
    }
    const result = await Notification.requestPermission();
    if (mountedRef.current) setPermission(result);
  }, []);

  return (
    <section className="settings-section">
      <h3 className="settings-section__title">Desktop notifications</h3>
      <label className="settings-checkbox">
        <input
          type="checkbox"
          checked={settings.enabled}
          onChange={(e) => setFlag('enabled', e.target.checked)}
          title={settings.enabled ? 'Disable browser notifications' : 'Enable browser notifications'}
          aria-label={settings.enabled ? 'Disable browser notifications' : 'Enable browser notifications'}
        />
        Enable browser notifications
      </label>
      <div className="settings-section__hint">Browser permission: {permission}</div>
      {permission === 'default' && (
        <button
          type="button"
          className="settings-inline-btn"
          onClick={requestPermission}
          title="Ask this browser to allow Phoenix notifications"
        >
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
          <input
            type="checkbox"
            checked={settings.notify_task_approval}
            onChange={(e) => setFlag('notify_task_approval', e.target.checked)}
            title={settings.notify_task_approval ? 'Stop notifying for task approvals' : 'Notify when a task needs approval'}
            aria-label={settings.notify_task_approval ? 'Stop notifying for task approvals' : 'Notify when a task needs approval'}
          />
          Task approval
        </label>
        <label className="settings-checkbox">
          <input
            type="checkbox"
            checked={settings.notify_question}
            onChange={(e) => setFlag('notify_question', e.target.checked)}
            title={settings.notify_question ? 'Stop notifying for questions' : 'Notify when the agent asks a question'}
            aria-label={settings.notify_question ? 'Stop notifying for questions' : 'Notify when the agent asks a question'}
          />
          Questions
        </label>
        <label className="settings-checkbox">
          <input
            type="checkbox"
            checked={settings.notify_error}
            onChange={(e) => setFlag('notify_error', e.target.checked)}
            title={settings.notify_error ? 'Stop notifying for errors and full context' : 'Notify for errors and full context'}
            aria-label={settings.notify_error ? 'Stop notifying for errors and full context' : 'Notify for errors and full context'}
          />
          Errors and context full
        </label>
        <label className="settings-checkbox">
          <input
            type="checkbox"
            checked={settings.notify_idle}
            onChange={(e) => setFlag('notify_idle', e.target.checked)}
            title={settings.notify_idle ? 'Stop notifying when long tasks finish' : 'Notify when long tasks finish'}
            aria-label={settings.notify_idle ? 'Stop notifying when long tasks finish' : 'Notify when long tasks finish'}
          />
          Long task finished
        </label>
      </div>
      {saving && <div className="settings-section__hint">Saving…</div>}
      {error && <div className="settings-section__error">{error}</div>}
    </section>
  );
}

function llmLanguageMeta(setting: LlmLanguageSetting, lang: string) {
  return setting.languages.find((entry) => entry.id === lang);
}

function llmLanguageLabel(setting: LlmLanguageSetting, lang: string): string {
  return llmLanguageMeta(setting, lang)?.label ?? lang;
}

function llmLanguageTooltip(setting: LlmLanguageSetting, lang: string): string {
  return llmLanguageMeta(setting, lang)?.description ?? lang;
}

/**
 * Global default LLM language. Applied only to NEW conversations; existing
 * conversations stay in whatever language they were created with, and chain
 * continuations / sub-agents inherit from their parent.
 */
function LlmLanguageSection({ onCloseMenu }: { onCloseMenu: () => void }) {
  const navigate = useNavigate();
  const [setting, setSetting] = useState<LlmLanguageSetting | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Monotonic save id (mirrors NotificationsSection): rapid clicks issue
  // multiple PUTs and we apply only the latest response, so out-of-order
  // completions don't overwrite the user's current selection.
  const latestSaveRef = useRef(0);
  // Live mirror of `setting`, read in the click handler so rapid clicks
  // (Phoenix → Caveman → Phoenix) compare against the value the previous
  // click just installed rather than a stale closure capture — without
  // putting side effects inside a setState updater.
  const settingRef = useRef<LlmLanguageSetting | null>(null);
  // Last server-confirmed value — used to roll back the optimistic
  // selection if the latest PUT fails.
  const confirmedRef = useRef<LlmLanguageSetting | null>(null);
  const mountedRef = useRef(true);

  useEffect(() => {
    mountedRef.current = true;
    return () => { mountedRef.current = false; };
  }, []);

  useEffect(() => {
    api.getLlmLanguageSetting()
      .then((loaded) => {
        if (!mountedRef.current) return;
        confirmedRef.current = loaded;
        settingRef.current = loaded;
        setSetting(loaded);
      })
      .catch((err) => {
        if (!mountedRef.current) return;
        setError(err instanceof Error ? err.message : 'Failed to load LLM language');
      });
  }, []);

  const select = useCallback((next: string) => {
    // Read the live value via ref, not a closure — captures stale state.
    const current = settingRef.current;
    if (!current || current.language === next) return;

    const saveId = latestSaveRef.current + 1;
    latestSaveRef.current = saveId;

    // Optimistic local update. Pure setState (no side effects inside the
    // updater); the PUT is fired from the handler scope.
    const optimistic = { ...current, language: next };
    settingRef.current = optimistic;
    setSetting(optimistic);
    setSaving(true);
    setError(null);

    api.updateLlmLanguageSetting(next)
      .then((saved) => {
        // Only the latest save's response is allowed to mutate state.
        // Earlier PUTs that arrive out of order are dropped on the floor.
        if (!mountedRef.current || latestSaveRef.current !== saveId) return;
        confirmedRef.current = saved;
        settingRef.current = saved;
        setSetting(saved);
        setSaving(false);
      })
      .catch((err: unknown) => {
        if (!mountedRef.current || latestSaveRef.current !== saveId) return;
        // Roll back to the last server-confirmed value so the UI matches the server.
        if (confirmedRef.current) {
          settingRef.current = confirmedRef.current;
          setSetting(confirmedRef.current);
        }
        setError(err instanceof Error ? err.message : 'Failed to save LLM language');
        setSaving(false);
      });
  }, []);

  if (!setting && !error) return null;

  return (
    <section className="settings-section">
      <h3 className="settings-section__title">LLM Language</h3>
      <div className="settings-section__hint">
        Sets the voice Phoenix uses with the model (system prompt, tool
        descriptions). Applies to new conversations only.
      </div>
      {setting && (
        <div
          className="settings-theme-row"
          role="radiogroup"
          aria-label="LLM language"
        >
          {setting.available.map((lang) => {
            const active = setting.language === lang;
            return (
              <button
                key={lang}
                type="button"
                role="radio"
                aria-checked={active}
                className={`settings-theme-btn${active ? ' active' : ''}`}
                onClick={() => select(lang)}
                disabled={saving}
                title={llmLanguageTooltip(setting, lang)}
              >
                {llmLanguageLabel(setting, lang)}
              </button>
            );
          })}
        </div>
      )}
      <button
        type="button"
        className="settings-inline-btn settings-about-link"
        onClick={() => { onCloseMenu(); navigate('/settings/llm-language'); }}
      >
        View prompts →
      </button>
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
