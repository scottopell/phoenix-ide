import { useState, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { api } from '../api';
import type { TaskEntry, TaskCountResponse } from '../api';
import { GroundingSection, GroundingState } from './GroundingPanel';
import { summarizeTasks, taskCountsLabel } from './groundingSummaries';
import './TasksPanel.css';

interface TasksPanelProps {
  conversationId: string | undefined;
  currentTaskId?: string | undefined;
  onTaskClick?: ((task: TaskEntry) => void) | undefined;
  expanded?: boolean;
  onToggleExpanded?: (expanded: boolean) => void;
  groupExpanded?: Record<string, boolean>;
  onGroupExpandedChange?: (expanded: Record<string, boolean>) => void;
  scrollTop?: number;
  onScrollTopChange?: (scrollTop: number) => void;
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

const DEFAULT_GROUP_EXPANDED: Record<string, boolean> = {
  'in-progress': true,
  ready: true,
  blocked: true,
  brainstorming: false,
  done: false,
  'wont-do': false,
};

export function TasksPanel({
  conversationId,
  currentTaskId,
  onTaskClick,
  expanded: controlledExpanded,
  onToggleExpanded,
  groupExpanded: controlledGroupExpanded,
  onGroupExpandedChange,
  scrollTop,
  onScrollTopChange,
}: TasksPanelProps) {
  const navigate = useNavigate();
  const [internalExpanded, setInternalExpanded] = useState(false);
  const expanded = controlledExpanded ?? internalExpanded;
  const setExpanded = onToggleExpanded ?? setInternalExpanded;
  const [tasks, setTasks] = useState<TaskEntry[]>([]);
  const [counts, setCounts] = useState<TaskCountResponse | null>(null);
  const [loading, setLoading] = useState(false);
  const [internalGroupExpanded, setInternalGroupExpanded] = useState(DEFAULT_GROUP_EXPANDED);
  const groupExpanded = controlledGroupExpanded ?? internalGroupExpanded;
  const setGroupExpanded = onGroupExpandedChange ?? setInternalGroupExpanded;

  // Clear both representations on conversation change so a stale count/list from
  // the prior conversation never bleeds across navigation (REQ-TASKS-UI-007).
  useEffect(() => {
    setTasks([]);
    setCounts(null);
  }, [conversationId]);

  // Collapsed-header counts: the lightweight count endpoint, fetched on mount so
  // the header carries its summary without the full list. Re-fetches when the
  // branch-derived current task changes so "current set" stays accurate.
  useEffect(() => {
    if (!conversationId) return;

    const controller = new AbortController();
    api
      .getConversationTaskCount(conversationId, currentTaskId, controller.signal)
      .then(setCounts)
      .catch((err) => {
        if (err.name !== 'AbortError') console.error('Failed to load task counts:', err);
      });

    return () => controller.abort();
  }, [conversationId, currentTaskId]);

  // Full task list: the expensive read (per-task slug mapping) is paid only when
  // the user expands the panel (REQ-TASKS-UI-007).
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
    setGroupExpanded({ ...groupExpanded, [status]: !groupExpanded[status] });
  };

  // Once the full list is loaded (expanded), it is authoritative for the header;
  // render through the same `taskCountsLabel`, so collapsing/expanding never
  // changes the wording.
  const fullSummary = summarizeTasks(tasks, currentTaskId);
  const headerCounts = expanded && tasks.length > 0 ? fullSummary : counts;
  const headerSummary = headerCounts
    ? taskCountsLabel(headerCounts, true)
    : conversationId
      ? 'loading…'
      : 'not loaded';

  return (
    <GroundingSection
      icon="☑"
      title="Tasks"
      summary={headerSummary}
      count={headerCounts?.active ?? 0}
      expanded={expanded}
      attention={(headerCounts?.current ?? false) || (headerCounts?.blocked ?? 0) > 0}
      onToggle={() => setExpanded(!expanded)}
      scrollTop={scrollTop}
      onScrollTopChange={onScrollTopChange}
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
                            key={task.id}
                            className={
                              'tasks-item-row'
                              + (isTerminal ? ' tasks-item-terminal' : '')
                              + (isCurrent ? ' tasks-item-current' : '')
                            }
                          >
                            <button
                              type="button"
                              className="tasks-item"
                              title={`${task.id}-${task.priority}-${task.status}--${task.slug}`}
                              onClick={() => onTaskClick?.(task)}
                            >
                              <span className={`tasks-pri ${PRIORITY_CLASS[task.priority] || 'tasks-pri-p3'}`}>
                                {task.priority}
                              </span>
                              <span className="tasks-id">{task.id}</span>
                              <span className="tasks-slug">{task.slug}</span>
                              {isCurrent && <span className="tasks-current-badge">current</span>}
                            </button>
                            {task.conversation_slug && !isCurrent && (
                              <button
                                type="button"
                                className="tasks-conv-link"
                                title="Go to conversation"
                                onClick={() => navigate(`/c/${task.conversation_slug}`)}
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
  // otherwise the lightweight counts drive it. Both carry the same shape and