import { useState, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { api } from '../api';
import type { TaskEntry } from '../api';
import { GroundingSection, GroundingState } from './GroundingPanel';
import { summarizeTasks } from './groundingSummaries';
import './TasksPanel.css';

interface TasksPanelProps {
  conversationId: string | undefined;
  currentTaskId?: string | undefined;
  onTaskClick?: ((task: TaskEntry) => void) | undefined;
}

const STATUS_ORDER: Record<string, number> = {
  'in-progress': 0,
  ready: 1,
  blocked: 2,
  brainstorming: 3,
  done: 4,
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
  const [groupExpanded, setGroupExpanded] = useState<Record<string, boolean>>({
    'in-progress': true,
    ready: true,
    blocked: true,
    brainstorming: false,
    done: false,
    'wont-do': false,
  });

  useEffect(() => {
    setTasks([]);
  }, [conversationId]);

  useEffect(() => {
    if (!conversationId) return;

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
  }, [conversationId]);

  const grouped = new Map<string, TaskEntry[]>();
  for (const task of tasks) {
    const group = grouped.get(task.status) || [];
    group.push(task);
    grouped.set(task.status, group);
  }

  const sortedGroups = [...grouped.entries()].toSorted(
    ([a], [b]) => (STATUS_ORDER[a] ?? 99) - (STATUS_ORDER[b] ?? 99),
  );

  const toggleGroup = (status: string) => {
    setGroupExpanded((prev) => ({ ...prev, [status]: !prev[status] }));
  };

  const summary = summarizeTasks(tasks, currentTaskId);

  return (
    <GroundingSection
      icon="☑"
      title="Tasks"
      summary={loading ? 'loading…' : summary.label}
      count={tasks.length > 0 ? summary.active : undefined}
      expanded={expanded}
      attention={summary.current || summary.blocked > 0}
      onToggle={() => setExpanded(!expanded)}
    >
      <div className={`tasks-panel${expanded ? ' is-expanded' : ''}`}>
        {loading && <GroundingState tone="loading">Loading tasks…</GroundingState>}
        {!loading && tasks.length === 0 && (
          <GroundingState>No tasks found for this project.</GroundingState>
        )}
        {!loading && tasks.length > 0 && (
          <div className="tasks-panel-body">
            {sortedGroups.map(([status, groupTasks]) => {
              const isTerminal = TERMINAL_STATUSES.has(status);
              const isOpen = groupExpanded[status] ?? !isTerminal;

              return (
                <div key={status} className="tasks-group">
                  <button
                    type="button"
                    className={`tasks-group-header${isTerminal ? ' tasks-group-terminal' : ''}`}
                    onClick={() => toggleGroup(status)}
                    aria-expanded={isOpen}
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
                            role="button"
                            tabIndex={0}
                            key={task.id}
                            className={
                              'tasks-item'
                              + (isTerminal ? ' tasks-item-terminal' : '')
                              + (isCurrent ? ' tasks-item-current' : '')
                            }
                            title={`${task.id}-${task.priority}-${task.status}--${task.slug}`}
                            onClick={() => onTaskClick?.(task)}
                            onKeyDown={(e) => {
                              if (e.key === 'Enter' || e.key === ' ') onTaskClick?.(task);
                            }}
                          >
                            <span className={`tasks-pri ${PRIORITY_CLASS[task.priority] || 'tasks-pri-p3'}`}>
                              {task.priority}
                            </span>
                            <span className="tasks-id">{task.id}</span>
                            <span className="tasks-slug">{task.slug}</span>
                            {isCurrent && <span className="tasks-current-badge">current</span>}
                            {task.conversation_slug && !isCurrent && (
                              <button
                                type="button"
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
    </GroundingSection>
  );
}
