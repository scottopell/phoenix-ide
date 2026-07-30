import type { QuotaDetails, RateLimitWindow } from '../sseSchemas';

// Window-minutes → human label. Codex uses 60 (hourly), 300 (five-hour),
// 1440 (daily), and 10080 (weekly); fall back to "Window" for anything unrecognized so a
// future limit type doesn't render an empty cell.
function windowLabel(windowMinutes: number | null): string {
  if (windowMinutes === 60) return 'Hourly';
  if (windowMinutes === 300) return '5-hour';
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

function creditsAreDepleted(type: QuotaDetails['rate_limit_reached_type']): boolean {
  return (
    type === 'workspace_owner_credits_depleted' ||
    type === 'workspace_member_credits_depleted'
  );
}

function SpendControlRow({ limit }: { limit: NonNullable<QuotaDetails['individual_limit']> }) {
  return (
    <div className="settings-codex-quota__credits">
      Individual limit: {limit.used} / {limit.limit} · {limit.remaining_percent}% remaining
      {formatReset(limit.resets_at) ? ` · ${formatReset(limit.resets_at)}` : null}
    </div>
  );
}

function exhaustionMessage(type: QuotaDetails['rate_limit_reached_type']): string | null {
  switch (type) {
    case 'rate_limit_reached':
      return 'Usage limit reached';
    case 'workspace_owner_usage_limit_reached':
      return 'Workspace usage limit reached';
    case 'workspace_member_usage_limit_reached':
      return 'Member usage limit reached';
    default:
      return null;
  }
}

function CreditsRow({ credits }: { credits: NonNullable<QuotaDetails['credits']> }) {
  if (credits.unlimited) {
    return <div className="settings-codex-quota__credits">Credits: Unlimited</div>;
  }
  if (!credits.has_credits) return null;
  const balance = credits.balance?.trim();
  return (
    <div className="settings-codex-quota__credits">
      Credits: {balance || 'Available'}
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
  const creditsDepleted = creditsAreDepleted(quota.rate_limit_reached_type);
  const reachedMessage = exhaustionMessage(quota.rate_limit_reached_type);
  const hasCreditsRow = creditsDepleted || (credits && (credits.unlimited || credits.has_credits));
  if (!quota.primary && !quota.secondary && quota.additional_limits.length === 0 && !hasCreditsRow && !quota.individual_limit && !reachedMessage) return null;
  return (
    <div className="settings-codex-quota">
      {quota.primary && (
        <QuotaRow label={windowLabel(quota.primary.window_minutes)} window={quota.primary} />
      )}
      {quota.secondary && (
        <QuotaRow label={windowLabel(quota.secondary.window_minutes)} window={quota.secondary} />
      )}
      {quota.additional_limits.map((family) => (
        <div key={family.limit_name}>
          {family.primary ? (
            <QuotaRow label={`${family.limit_name} · ${windowLabel(family.primary.window_minutes)}`} window={family.primary} />
          ) : null}
          {family.secondary ? (
            <QuotaRow label={`${family.limit_name} · ${windowLabel(family.secondary.window_minutes)}`} window={family.secondary} />
          ) : null}
        </div>
      ))}
      {quota.individual_limit ? <SpendControlRow limit={quota.individual_limit} /> : null}
      {reachedMessage ? <div className="settings-codex-quota__credits">{reachedMessage}</div> : null}
      {creditsDepleted ? (
        <div className="settings-codex-quota__credits">No credits remaining</div>
      ) : credits ? (
        <CreditsRow credits={credits} />
      ) : null}
    </div>
  );
}
