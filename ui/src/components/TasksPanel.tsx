import { useState, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { api } from '../api';
import type { TaskEntry } from '../api';
import './TasksPanel.css';

interface TasksPanelProps {
  conversationId: string | undefined;
  /** Task ID of the current conversation's task (for Work mode highlight) */
  currentTaskId?: string | undefined;
  /** Called when a task is clicked for detail view */
  onTaskClick?: ((task: TaskEntry) => void) | undefined;
}

const STATUS_ORDER: Record<string, number> = {
  'in-progress': 0,
  'ready': 1,
  'blocked': 2,
  'brainstorming': 3,
  'done': 4,
  'wont-do': 5,
};

const PRIORITY_CLASS: Record<string, string> = {
  p0: 'tasks-pri-p0',
  p1: 'tasks-pri-p1',
  p2: 'tasks-pri-p2',
  p3: 'tasks-pri-p3',
  p4: 'tasks-pri-p4',
};

const TERMINAL_STATUSES = new Set(['done', 'wont-do']);

export function TasksPanel({ conversationId, currentTaskId, onTaskClick }: TasksPanelProps) {
  const navigate = useNavigate();
  const [expanded, setExpanded] = useState(false);
  const [tasks, setTasks] = useState<TaskEntry[]>([]);
  const [loading, setLoading] = useState(false);

  // Track which status groups are expanded (active groups default open, terminal closed)
  const [groupExpanded, setGroupExpanded] = useState<Record<string, boolean>>({
    'in-progress': true,
    'ready': true,
    'blocked': true,
    'brainstorming': false,
    'done': false,
    'wont-do': false,
  });

  useEffect(() => {
    if (!conversationId || !expanded) return;

    const controller = new AbortController();
    setLoading(true);
    api
      .listConversationTasks(conversationId, controller.signal)
      .then((resp) => setTasks(resp.tasks))
      .catch((err) => {
        if (err.name !== 'AbortError') console.error('Failed to load tasks:', err);
      })
      .finally(() => setLoading(false));

    return () => controller.abort();
  }, [conversationId, expanded]);

  // Group tasks by status
  const grouped = new Map<string, TaskEntry[]>();
  for (const task of tasks) {
    const group = grouped.get(task.status) || [];
    group.push(task);
    grouped.set(task.status, group);
  }

  // Sort groups by STATUS_ORDER
  const sortedGroups = [...grouped.entries()].sort(
    ([a], [b]) => (STATUS_ORDER[a] ?? 99) - (STATUS_ORDER[b] ?? 99)
  );

  const activeCount = tasks.filter((t) => !TERMINAL_STATUSES.has(t.status)).length;
  const terminalCount = tasks.filter((t) => TERMINAL_STATUSES.has(t.status)).length;

  const toggleGroup = (status: string) => {
    setGroupExpanded((prev) => ({ ...prev, [status]: !prev[status] }));
  };

  return (
    <div className="tasks-panel">
      <button
        className="tasks-panel-header"
        onClick={() => setExpanded(!expanded)}
      >
        <span className={`tasks-panel-chevron${expanded ? ' expanded' : ''}`}>
          &#9654;
        </span>
        <span className="tasks-panel-summary">
          Tasks
          {tasks.length > 0 && (
            <>
              {' '}&middot; {activeCount} active
              {terminalCount > 0 && (
                <span className="tasks-done-count"> &middot; {terminalCount} closed</span>
              )}
            </>
          )}
        </span>
      </button>

      {expanded && (
        <div className="tasks-panel-body">
          {loading && <div className="tasks-loading">Loading...</div>}
          {!loading && tasks.length === 0 && (
            <div className="tasks-empty">No tasks/ directory found</div>
          )}
          {!loading &&
            sortedGroups.map(([status, groupTasks]) => {
              const isTerminal = TERMINAL_STATUSES.has(status);
              const isOpen = groupExpanded[status] ?? !isTerminal;

              return (
                <div key={status} className="tasks-group">
                  <button
                    className={`tasks-group-header${isTerminal ? ' tasks-group-terminal' : ''}`}
                    onClick={() => toggleGroup(status)}
                  >
                    <span className={`tasks-group-chevron${isOpen ? ' expanded' : ''}`}>
                      &#9654;
                    </span>
                    <span className={`tasks-status-dot tasks-status-${status}`} />
                    <span className="tasks-group-label">{status}</span>
                    <span className="tasks-group-count">({groupTasks.length})</span>
                  </button>
                  {isOpen && (
                    <div className="tasks-group-items">
                      {groupTasks.map((task) => {
                        const isCurrent = currentTaskId === task.id;
                        return (
                          <div
                            key={task.id}
                            className={
                              'tasks-item'
                              + (isTerminal ? ' tasks-item-terminal' : '')
                              + (isCurrent ? ' tasks-item-current' : '')
                            }
                            title={`${task.id}-${task.priority}-${task.status}--${task.slug}`}
                            onClick={() => onTaskClick?.(task)}
                          >
                            <span className={`tasks-pri ${PRIORITY_CLASS[task.priority] || 'tasks-pri-p3'}`}>
                              {task.priority}
                            </span>
                            <span className="tasks-id">{task.id}</span>
                            <span className="tasks-slug">{task.slug}</span>
                            {isCurrent && <span className="tasks-current-badge">current</span>}
                            {task.conversation_slug && !isCurrent && (
                              <button
                                className="tasks-conv-link"
                                title="Go to conversation"
                                onClick={(e) => {
                                  e.stopPropagation();
                                  navigate(`/c/${task.conversation_slug}`);
                                }}
                              >
                                &rarr;
                              </button>
                            )}
                          </div>
                        );
                      })}
                    </div>
                  )}
                </div>
              );
            })}
        </div>
      )}
    </div>
  );
}
