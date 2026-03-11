/**
 * FirstTaskWelcome Component
 *
 * Brief welcome modal shown when the first task is created in a project.
 * Explains the tasks/ directory and links to taskmd tooling.
 */

interface FirstTaskWelcomeProps {
  visible: boolean;
  onClose: () => void;
}

export function FirstTaskWelcome({ visible, onClose }: FirstTaskWelcomeProps) {
  if (!visible) return null;

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div
        className="modal first-task-welcome"
        onClick={(e) => e.stopPropagation()}
      >
        <h3>Task Created</h3>
        <p className="first-task-welcome-text">
          Phoenix uses a <code>tasks/</code> directory to track work plans. You,
          other developers, and other tools can read and create task files too.
        </p>
        <p className="first-task-welcome-link">
          See{' '}
          <a
            href="https://github.com/scottopell/taskmd"
            target="_blank"
            rel="noopener noreferrer"
          >
            taskmd
          </a>{' '}
          for standalone tooling.
        </p>
        <div className="modal-actions">
          <button className="btn-primary" onClick={onClose}>
            Got it
          </button>
        </div>
      </div>
    </div>
  );
}
