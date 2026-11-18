import { useCoordinatorQuestionCount } from '../coordinator/coordinatorQuestions';
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
  // REQ-AUQ-011: an open question from Ember is something the user owes.
  const openQuestions = useCoordinatorQuestionCount();
  const questionLabel = openQuestions === 0
    ? null
    : `${openQuestions} question${openQuestions === 1 ? '' : 's'} for you`;
  const statusLabel = [questionLabel, unreadLabel].filter(Boolean).join(', ') || null;
  return (
    <button
      type="button"
      className={`ember-entry${active ? ' active' : ''}${compact ? ' compact' : ''}${unreadLabel ? ' unread' : ''}${questionLabel ? ' has-questions' : ''}`}
      onClick={onClick}
      title={compact ? `Open Ember — Global coordinator${statusLabel ? ` (${statusLabel})` : ''}` : undefined}
      aria-label={statusLabel ? `Open Ember, ${statusLabel}` : 'Open Ember'}
      aria-description="Global coordinator"
      aria-current={active ? 'page' : undefined}
    >
      <span className="ember-entry-mark" aria-hidden="true">
        ✦
        {questionLabel
          ? <span className="ember-entry-question-dot" title={questionLabel} />
          : unreadLabel && <span className={`ember-entry-unread-dot${unread === 'marked' ? ' ember-entry-unread-dot--marked' : ''}`} title={unreadLabel} />}
      </span>
      {!compact && (
        <span className="ember-entry-copy">
          <strong>Ember</strong>
          <small>{questionLabel ?? 'Global coordinator'}</small>
        </span>
      )}
      {!compact && <span className="ember-entry-arrow" aria-hidden="true">→</span>}
    </button>
  );
}
