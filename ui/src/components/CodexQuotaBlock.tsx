import type { QuotaDetails, RateLimitWindow } from '../sseSchemas';

// Window-minutes → human label. Codex uses 60 (hourly), 1440 (daily),
// 10080 (weekly); fall back to "Window" for anything unrecognized so a
// future limit type doesn't render an empty cell.
function windowLabel(windowMinutes: number | null): string {
  if (windowMinutes === 60) return 'Hourly';
  if (windowMinutes === 1440) return 'Daily';
  if (windowMinutes === 10080) return 'Weekly';
  return 'Window';
}

function fillClass(pct: number): string {
  if (pct >= 85) return 'settings-codex-quota__fill settings-codex-quota__fill--high';
  if (pct >= 60) return 'settings-codex-quota__fill settings-codex-quota__fill--mid';
  return 'settings-codex-quota__fill settings-codex-quota__fill--low';
}

// Unix seconds → "3:42 PM" today, "Mar 3, 3:42 PM" otherwise. Matches the
// terse Phoenix UI style; the codex CLI's verbose `Mar 3rd, 2026 3:42 PM`
// is too long for the 240px-wide settings dropdown.
function formatReset(resetAt: number | null): string | null {
  if (resetAt === null) return null;
  const d = new Date(resetAt * 1000);
  if (Number.isNaN(d.getTime())) return null;
  const today = new Date();
  const sameDay =
    d.getFullYear() === today.getFullYear() &&
    d.getMonth() === today.getMonth() &&
    d.getDate() === today.getDate();
  const time = d.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });
  if (sameDay) return `resets ${time}`;
  const date = d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
  return `resets ${date} ${time}`;
}

function QuotaRow({ label, window }: { label: string; window: RateLimitWindow }) {
  const pct = Math.max(0, Math.min(100, window.used_percent));
  const reset = formatReset(window.resets_at);
  return (
    <div className="settings-codex-quota__row">
      <span className="settings-codex-quota__label">{label}</span>
      <span className="settings-codex-quota__bar">
        <span className={fillClass(pct)} style={{ width: `${pct}%` }} />
      </span>
      <span className="settings-codex-quota__pct">{pct.toFixed(0)}%</span>
      {reset && <span className="settings-codex-quota__reset">{reset}</span>}
    </div>
  );
}

/// Renders the structured codex quota snapshot — per-window usage bars,
/// credits state. Returns `null` when the snapshot has no displayable
/// data so callers can render unconditionally.
///
/// Shared between SettingsDropdown.CodexSection and ErrorBanner (when
/// the conversation hits a terminal `usage_limit_reached` state).
export function CodexQuotaBlock({ quota }: { quota: QuotaDetails }) {
  const credits = quota.credits;
  if (!quota.primary && !quota.secondary && !credits) return null;
  return (
    <div className="settings-codex-quota">
      {quota.primary && (
        <QuotaRow label={windowLabel(quota.primary.window_minutes)} window={quota.primary} />
      )}
      {quota.secondary && (
        <QuotaRow label={windowLabel(quota.secondary.window_minutes)} window={quota.secondary} />
      )}
      {credits && credits.has_credits && credits.balance && (
        <div className="settings-codex-quota__credits">
          Credits: {credits.balance}{credits.unlimited ? ' (unlimited)' : ''}
        </div>
      )}
      {credits && !credits.has_credits && (
        <div className="settings-codex-quota__credits">No credits remaining</div>
      )}
    </div>
  );
}
