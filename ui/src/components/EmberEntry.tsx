import './EmberEntry.css';

export function EmberEntry({
  active = false,
  compact = false,
  onClick,
}: {
  active?: boolean;
  compact?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className={`ember-entry${active ? ' active' : ''}${compact ? ' compact' : ''}`}
      onClick={onClick}
      title={compact ? 'Open Ember — Global coordinator' : undefined}
      aria-label="Open Ember, Global coordinator"
      aria-current={active ? 'page' : undefined}
    >
      <span className="ember-entry-mark" aria-hidden="true">✦</span>
      {!compact && <span className="ember-entry-copy"><strong>Ember</strong><small>Global coordinator</small></span>}
      {!compact && <span className="ember-entry-arrow" aria-hidden="true">→</span>}
    </button>
  );
}
