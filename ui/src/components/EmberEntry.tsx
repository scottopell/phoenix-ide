import './EmberEntry.css';

export function EmberEntry({
  active = false,
  compact = false,
  unread = null,
  onClick,
}: {
  active?: boolean;
  compact?: boolean;
  /** Global conversation read state; `null` when read or unknown. */
  unread?: 'marked' | 'new_activity' | null;
  onClick: () => void;
}) {
  const unreadLabel = unread === 'marked' ? 'Marked unread' : unread === 'new_activity' ? 'New activity' : null;
  return (
    <button
      type="button"
      className={`ember-entry${active ? ' active' : ''}${compact ? ' compact' : ''}${unreadLabel ? ' unread' : ''}`}
      onClick={onClick}
      title={compact ? `Open Ember — Global coordinator${unreadLabel ? ` (${unreadLabel})` : ''}` : undefined}
      aria-label={unreadLabel ? `Open Ember, ${unreadLabel}` : 'Open Ember'}
      aria-description="Global coordinator"
      aria-current={active ? 'page' : undefined}
    >
      <span className="ember-entry-mark" aria-hidden="true">
        ✦
        {unreadLabel && <span className={`ember-entry-unread-dot${unread === 'marked' ? ' ember-entry-unread-dot--marked' : ''}`} title={unreadLabel} />}
      </span>
      {!compact && <span className="ember-entry-copy"><strong>Ember</strong><small>Global coordinator</small></span>}
      {!compact && <span className="ember-entry-arrow" aria-hidden="true">→</span>}
    </button>
  );
}
